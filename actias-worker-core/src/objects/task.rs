//! The object task: one call at a time out of the mailbox, the
//! guarded dispatch around each, alarms, teardown and the call chain
//! that refuses cycles.

use super::*;

/// Everything configurable about one pinned task; the runtime is the
/// only required piece.
#[derive(Default)]
pub struct TaskOptions {
    /// Seconds one dispatched call may run; [`None`] leaves calls unbounded.
    pub call_budget: Option<u64>,
    /// The object's durable half; [`None`] leaves state in-memory only.
    pub storage: Option<crate::storage::SqliteStorage>,
    /// Idle time after which the task hibernates: it simply ends, state
    /// already on disk, and the host revives it on the next touch. An
    /// object holding a pending alarm stays warm until it fires.
    pub hibernate_after: Option<std::time::Duration>,
    /// The output gate: runs after any call that wrote, before its caller
    /// hears the result. Snapshot shipping lives here.
    pub after_write: Option<AfterWrite>,
    /// The registry mirror for this object's alarm; [`None`] keeps alarms
    /// file-local (tests, embedded runs).
    pub alarm_sync: Option<AlarmSync>,
    /// Delivery limits for `__queue` instances; the default is the
    /// production policy.
    pub queue: crate::platform::queue::QueuePolicy,
    /// The deletion sequence, run when a call asked `state:destroy()`;
    /// [`None`] ends the task without platform cleanup (tests, embedded
    /// runs).
    pub destroy: Option<DestroyFn>,
    /// Restates the claim for a class with a declared lifespan; [`None`]
    /// for classes that never expire.
    pub keep_claimed: Option<KeepClaimed>,
    /// Milliseconds a `directory` function may run. Separate from
    /// [`TaskOptions::call_budget`] because the caller did not ask for
    /// this work, and it runs on the object's pinned task, so it also
    /// bounds the mailbox stall. [`None`] takes the kernel default.
    pub directory_budget_ms: Option<u64>,
}

/// Moves `runtime` onto its own task forever and hands back its mailbox.
///
/// The task ends when every handle is dropped; the vm drops with it.
pub fn spawn_object_task(runtime: ActiasRuntime, options: TaskOptions) -> ObjectHandle {
    use crate::extensions::objects::PendingAlarm;

    let TaskOptions {
        call_budget,
        mut storage,
        hibernate_after,
        after_write,
        alarm_sync,
        queue,
        destroy,
        keep_claimed,
        directory_budget_ms,
    } = options;
    let directory_budget_ms =
        directory_budget_ms.unwrap_or(crate::directory::DEFAULT_EVAL_BUDGET_MS);

    let (sender, mut receiver) = mpsc::channel::<ObjectCall>(MAILBOX_DEPTH);

    // A persisted alarm re-arms the moment the object is resident again;
    // past-due fires immediately. (A cold object with a due alarm still
    // needs a touch to wake, until placement can scan.)
    let pending = storage.as_mut().and_then(|storage| {
        storage
            .load_alarm()
            .ok()
            .flatten()
            .map(|(due_ms, class, name, own_key)| PendingAlarm {
                due_ms,
                class,
                name,
                own_key,
            })
    });

    let home = Arc::new(ObjectHome::new(
        storage,
        pending,
        queue,
        runtime
            .app_data_ref::<Arc<crate::runtime::PreparedRevision>>()
            .map(|revision| revision.clone()),
        alarm_sync,
    ));
    // The file is the truth at spawn: mirroring it (arm or clear) heals a
    // registry row lost to a crash or left stale by a fired-and-died
    // holder, so a wake self-corrects instead of looping forever.
    home.mirror_alarm(
        home.pending_alarm()
            .as_ref()
            .map(|pending_alarm| pending_alarm.due_ms),
    );
    // Undelivered stream events from a previous residency re-arm the
    // pump immediately; edges and cursors are rows, so the file is the
    // truth here too.
    if home.has_storage()
        && let Ok(due) = home.with_storage(crate::streams::next_delivery_due)
    {
        home.set_delivery_due(due);
    }
    runtime.set_app_data(home.clone());

    // The pinned vm's identity, when the host set it; names the span
    // every dispatched call runs under, so a trace reads
    // "Channel/general.post" instead of a bare method.
    let span_prefix = runtime
        .app_data_ref::<crate::streams::PublisherIdentity>()
        .map(|id| format!("{}/{}.", id.class, id.name))
        .unwrap_or_default();

    tokio::spawn(async move {
        // Popping only after the previous call finished is the input gate;
        // there is deliberately no concurrency inside this loop. A due
        // alarm is just one more message source, so it serializes with
        // calls exactly like they serialize with each other.
        loop {
            let pending = home.pending_alarm();
            let delivery = home.delivery_due();

            // The earliest of the app's alarm and the stream delivery
            // timer wakes the task; either keeps the vm warm, because
            // hibernating past due work would silently drop it.
            let alarm_due = pending.as_ref().map(|alarm| alarm.due_ms);
            let wake_due = match (alarm_due, delivery) {
                (Some(alarm), Some(delivery)) => Some(alarm.min(delivery)),
                (a, d) => a.or(d),
            };

            let call = if let Some(due) = wake_due {
                let wait = (due - crate::extensions::objects::unix_now_ms()).max(0);
                tokio::select! {
                    call = receiver.recv() => match call {
                        Some(call) => call,
                        None => break,
                    },
                    _ = tokio::time::sleep(std::time::Duration::from_millis(wait as u64)) => {
                        let deliver_first = delivery.is_some_and(|d| alarm_due.is_none_or(|a| d <= a));
                        if deliver_first {
                            home.take_delivery_due();
                            // Platform-initiated work roots its own trace,
                            // named for why it ran.
                            let span = actias_common::tracing::info_span!(
                                "stream delivery",
                                otel.name = %format!("deliver {span_prefix}events"),
                                otel.kind = "internal",
                            );
                            actias_common::tracing::Instrument::instrument(
                                crate::streams::pump(&runtime, &home),
                                span,
                            )
                            .await;
                        } else if let Some(alarm) = pending {
                            let span = actias_common::tracing::info_span!(
                                "alarm",
                                otel.name = %format!("alarm {}", alarm.own_key),
                                otel.kind = "internal",
                            );
                            actias_common::tracing::Instrument::instrument(
                                fire_alarm(
                                    &runtime,
                                    &home,
                                    alarm,
                                    call_budget,
                                    after_write.as_ref(),
                                    directory_budget_ms,
                                ),
                                span,
                            )
                            .await;
                        }
                        if let Some(keep) = keep_claimed.as_ref()
                            && home.take_refresh_due()
                        {
                            keep();
                        }
                        if home.destroy_requested() {
                            destroy_teardown(&mut receiver, destroy.as_ref()).await;
                            break;
                        }
                        continue;
                    }
                }
            } else if let Some(idle) = hibernate_after {
                tokio::select! {
                    call = receiver.recv() => match call {
                        Some(call) => call,
                        None => break,
                    },
                    // Hibernation is just ending: the file is the state,
                    // and the host revives on the next touch.
                    _ = tokio::time::sleep(idle) => break,
                }
            } else {
                match receiver.recv().await {
                    Some(call) => call,
                    None => break,
                }
            };

            // The dispatch runs as a child of the caller's span, so the
            // whole causal chain (request, object, its kv and sql, the
            // objects it calls) reads as one trace.
            let span = actias_common::tracing::info_span!(
                parent: &call.span,
                "object call",
                otel.name = %format!("{span_prefix}{}", call.method),
                otel.kind = "internal",
            );
            let Dispatched { result, gate } = actias_common::tracing::Instrument::instrument(
                guarded_dispatch(
                    &runtime,
                    &home,
                    &call.method,
                    call.payload,
                    call_budget,
                    after_write.as_ref(),
                    directory_budget_ms,
                ),
                span,
            )
            .await;

            // A caller that stopped waiting is its own problem; the state
            // change it asked for has already happened either way.
            match gate {
                // The output gate waits off this task, so the next call
                // runs while this answer is held. That is what keeps the
                // gate a latency cost rather than a throughput one: a
                // burst of writes rides one shipping flight and their
                // answers release together. Order is the mailbox's
                // property and is untouched by answering out of it.
                Some(gate) => {
                    let reply = call.reply;
                    tokio::spawn(async move {
                        let answer = match gate.await {
                            Ok(()) => result,
                            Err(error) => Err(ObjectError::NotDurable(error)),
                        };
                        let _ = reply.send(answer);
                    });
                }
                None => {
                    let _ = call.reply.send(result);
                }
            }

            // A busy object is not abandoned: restating the claim keeps
            // its lifespan measuring idleness, not residency age.
            if let Some(keep) = keep_claimed.as_ref()
                && home.take_refresh_due()
            {
                keep();
            }

            if home.destroy_requested() {
                destroy_teardown(&mut receiver, destroy.as_ref()).await;
                break;
            }
        }
    });

    ObjectHandle { sender }
}

/// Ends a destroyed object's residency: the destroying call's answer is
/// already on its way, everything still queued is refused, and the
/// platform's cleanup runs once the box is empty. New sends refuse when
/// the channel closes with the task.
pub(super) async fn destroy_teardown(
    receiver: &mut mpsc::Receiver<ObjectCall>,
    destroy: Option<&DestroyFn>,
) {
    receiver.close();
    while let Some(queued) = receiver.recv().await {
        let _ = queued.reply.send(Err(ObjectError::Call(
            "The object was destroyed.".to_owned(),
        )));
    }
    if let Some(destroy) = destroy
        && let Err(error) = destroy().await
    {
        actias_common::tracing::warn!(
            %error,
            "deletion sequence incomplete; the janitor finishes from the tombstone"
        );
    }
}

/// Runs one due alarm: cleared before dispatch, so a handler that sets the
/// next alarm is not clobbered afterwards. An alarm is best-effort work the
/// object asked itself for; its failure is logged, never propagated.
pub(super) async fn fire_alarm(
    runtime: &ActiasRuntime,
    home: &ObjectHome,
    alarm: crate::extensions::objects::PendingAlarm,
    call_budget: Option<u64>,
    after_write: Option<&AfterWrite>,
    directory_budget_ms: u64,
) {
    home.clear_alarm();

    // Platform classes dispatch in rust and keep the plain spelling;
    // Lua classes take the internal `__alarm`, which resolves the hook
    // (handles refuse that spelling, so it is platform-originated).
    let method = if alarm.class.starts_with("__") {
        "alarm"
    } else {
        "__alarm"
    };
    // The gate is dropped rather than awaited: an alarm has no caller to
    // answer, so there is no acknowledgment for durability to protect,
    // and holding the mailbox for a round trip would throttle the
    // alarm-driven platform classes (queue delivery above all). The
    // write still ships, and a failure to ship is logged by the shipper.
    let Dispatched { result, gate: _ } = guarded_dispatch(
        runtime,
        home,
        "__dispatch",
        serde_json::json!({
            "class": alarm.class,
            "method": method,
            "name": alarm.name,
            "args": [],
            "chain": [alarm.own_key],
        }),
        call_budget,
        after_write,
        directory_budget_ms,
    )
    .await;

    if let Err(error) = result {
        actias_common::tracing::warn!(%error, "object alarm failed");
    }
}

/// One dispatched call, fully guarded: its own budget and its own
/// transaction (a failed method persists nothing partial).
/// Derives and stores the object's directory row, containing every
/// failure. Called only after a successful call that wrote, on the
/// call's own connection so the row commits with it.
///
/// The budget is separate from the caller's call budget, because the
/// caller did not ask for this work; it still runs on the object's
/// pinned task, so it also bounds the mailbox stall.
pub(super) fn record_directory(runtime: &ActiasRuntime, home: &ObjectHome, budget_ms: u64) {
    let Some((class, name)) = runtime
        .app_data_ref::<crate::extensions::objects::CurrentDispatch>()
        .map(|current| (current.class.clone(), current.name.clone()))
    else {
        return;
    };

    runtime.begin_short_budget(budget_ms);
    let derived = crate::extensions::objects::derive_directory(runtime, &class, &name);
    runtime.end_call_budget();

    // A class with no directory declaration: nothing to record, and
    // nothing to mark either.
    let Some(derived) = derived else { return };

    // The version publish minted for this class's declared field set.
    // Absent for a revision published before fields were declared:
    // zero is honest rather than a placeholder, and the merge order
    // treats it as the lowest, so any row a versioned publish later
    // derives supersedes these.
    let declaration = runtime
        .app_data_ref::<Arc<crate::runtime::PreparedRevision>>()
        .and_then(|revision| revision.directory_spec(&class));
    let dver = declaration.as_ref().map_or(0, |spec| spec.dver as i64);

    // A row carrying a field the class did not declare, or a value of
    // a different kind than declared, is contained exactly like a
    // throw: the business write commits, the row keeps its last good
    // value, and the failure is marked. Serving it instead would put a
    // value in a column bound to a different kind, which is how a
    // comparison silently answers wrong.
    let derived = match (derived, declaration.as_ref()) {
        (Ok(row), Some(spec)) => crate::directory::evaluate::conform(&row, spec).map(|()| row),
        (derived, _) => derived,
    };

    let stored = match derived {
        Ok(row) => home.with_storage(|storage| crate::directory::row::record(storage, dver, &row)),
        Err(why) => {
            actias_common::tracing::warn!(
                class = %class,
                name = %name,
                error = %why,
                "the directory row could not be derived; keeping the last good row"
            );
            home.with_storage(|storage| crate::directory::row::record_failure(storage, dver))
        }
    };

    if let Err(error) = stored {
        // The row is an index over state that is already durable, so a
        // failure to write it costs freshness, never correctness: the
        // repair paths re-derive it from the shipped copy.
        actias_common::tracing::warn!(
            class = %class,
            name = %name,
            %error,
            "the directory row could not be stored"
        );
    }
}

pub(super) async fn guarded_dispatch(
    runtime: &ActiasRuntime,
    home: &ObjectHome,
    method: &str,
    payload: serde_json::Value,
    call_budget: Option<u64>,
    after_write: Option<&AfterWrite>,
    directory_budget_ms: u64,
) -> Dispatched {
    // The platform's own end-of-life verb: handles refuse every "__"
    // spelling, so its arrival here is provably platform-originated.
    // The answer goes out first; the task tears down after it.
    if payload.get("method").and_then(|m| m.as_str()) == Some("__destroy") {
        home.request_destroy();
        return Dispatched {
            result: Ok(serde_json::Value::Null),
            gate: None,
        };
    }

    let has_storage = home.has_storage();

    if has_storage && let Err(error) = home.with_storage(|storage| storage.begin()) {
        return Dispatched {
            result: Err(ObjectError::Call(format!(
                "The call's transaction could not open: {error}"
            ))),
            gate: None,
        };
    }

    if let Some(seconds) = call_budget {
        runtime.begin_call_budget(seconds);
    }
    // Platform-implemented classes never enter the vm; everything else is
    // the Lua dispatch, user classes and Lua-bodied platform classes alike.
    let result = if crate::platform::handles(method, &payload) {
        crate::platform::dispatch(runtime, home, payload).await
    } else {
        dispatch(runtime, method, payload).await
    };
    runtime.end_call_budget();

    // The directory row, derived before the commit so it rides the
    // call's own transaction: the row and the state it describes can
    // never disagree, because there is nothing to disagree between.
    // Contained on purpose, and this is the rule the whole feature
    // rests on: a derived index may never veto a business write. A
    // throw, a blown budget, a shape the kernel refuses, all keep the
    // last good row, mark the failure for the console and backfill,
    // and let the call answer normally.
    if has_storage && result.is_ok() && home.wrote_since_mark() {
        record_directory(runtime, home, directory_budget_ms);
    }

    let mut gate = None;

    if has_storage {
        match &result {
            Ok(_) => {
                if let Err(error) = home.with_storage(|storage| storage.commit()) {
                    return Dispatched {
                        result: Err(ObjectError::Call(format!(
                            "The call's writes could not commit: {error}"
                        ))),
                        gate: None,
                    };
                }
            }
            Err(_) => {
                if let Err(error) = home.with_storage(|storage| storage.rollback()) {
                    actias_common::tracing::warn!(%error, "rollback failed");
                }
                home.resync_alarm_from_storage();
            }
        }

        // No checkpoint here: synchronous=FULL already fsynced the WAL
        // frame at commit, so a per-call TRUNCATE would fold the log on
        // every call and buy no durability. The shipper owns
        // checkpoints, and an implicit fold mid-flight would move bytes
        // its frame reader is about to ship.

        // The output gate: calling the hook starts the write on its way,
        // and the future it hands back is what the answer waits behind.
        // A failed call is not gated, only shipped: a rolled-back call
        // acknowledges nothing, so there is nothing for durability to
        // protect and no reason to make its error wait.
        if let Some(after_write) = after_write
            && home.writes_advanced()
        {
            let waiting = after_write();
            if result.is_ok() {
                gate = Some(waiting);
            }
        }
    }

    Dispatched { result, gate }
}

/// How deep object calls may nest. Cycles are refused below; this is the
/// other unbounded shape, distinct objects each costing a mailbox and
/// possibly a network forward. Real designs nest a handful deep.
pub const MAX_CALL_DEPTH: usize = 32;

/// Extends a call chain onto `key`, refusing cycles and runaway depth.
///
/// Every routed call carries the keys already on its stack; a target that
/// appears there would deadlock on its own busy mailbox, so it is refused
/// loudly instead.
///
/// # Errors
/// Returns the cycle or the depth spelled out, for the script author.
pub fn extend_call_chain(chain: &[String], key: &str) -> Result<Vec<String>, String> {
    if chain.iter().any(|entry| entry == key) {
        return Err(format!(
            "Reentrant object call refused: {} -> {key} would deadlock.",
            chain.join(" -> "),
        ));
    }

    if chain.len() >= MAX_CALL_DEPTH {
        return Err(format!(
            "Object calls nested {} deep, past the limit of {MAX_CALL_DEPTH}: {} -> {key}.",
            chain.len(),
            chain.join(" -> "),
        ));
    }

    let mut child = chain.to_vec();
    child.push(key.to_owned());
    Ok(child)
}

/// Runs one method against the vm: json in, json out.
pub(super) async fn dispatch(
    runtime: &ActiasRuntime,
    method: &str,
    payload: serde_json::Value,
) -> Result<serde_json::Value, ObjectError> {
    let function: mlua::Function = runtime
        .globals()
        .get(method)
        .map_err(|_| ObjectError::Call(format!("Object has no method '{method}'.")))?;

    let argument = runtime
        .to_value(&payload)
        .map_err(|e| ObjectError::Call(e.to_string()))?;

    let value: mlua::Value = function
        .call_async(argument)
        .await
        .map_err(|e| ObjectError::Call(e.to_string()))?;

    runtime
        .from_value(value)
        .map_err(|e| ObjectError::Call(e.to_string()))
}
