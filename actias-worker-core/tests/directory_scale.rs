//! The directory's costs at size, measured rather than estimated: what a
//! fold, an overlay rebuild and a query cost at 10k, 100k and 1M rows.
//! Ignored by default because the largest case takes a while; run with
//!
//!     cargo test -p actias-worker-core --release --test directory_scale -- --ignored --nocapture
use actias_worker_core::directory::compact;
use actias_worker_core::directory::delta::{self, DeltaRow};
use actias_worker_core::directory::manifest::{Field, Manifest};
use actias_worker_core::directory::overlay::{Overlay, Query};
use actias_worker_core::directory::predicate::{Compare, Condition, Order, Where};
use actias_worker_core::directory::row::{Pair, RowSnapshot};
use actias_worker_core::directory::shape::Value;
use std::time::Instant;

fn row(i: u64, rev: i64) -> DeltaRow {
    let pair = |field: &str, kind: &str, value: String| Pair {
        field: field.to_owned(),
        kind: kind.to_owned(),
        value,
    };
    DeltaRow {
        object_id: blake3::hash(&i.to_le_bytes()).to_hex().to_string(),
        name: format!("lot-{i:08}"),
        epoch: 3,
        snapshot: RowSnapshot {
            rev,
            dver: 1,
            fields: vec![
                pair(
                    "status",
                    "string",
                    if i.is_multiple_of(7) {
                        "open".into()
                    } else {
                        "closed".into()
                    },
                ),
                pair("high_bid", "integer", ((i * 37) % 10_000).to_string()),
                pair("owner", "string", format!("user-{}", i % 5000)),
                pair(
                    "title",
                    "string",
                    format!("A fairly ordinary lot number {i} with a title of typical length"),
                ),
            ],
            failed: None,
        },
        tombstone: false,
    }
}

fn manifest() -> Manifest {
    let field = |name: &str, kind: &str| Field {
        name: name.into(),
        kind: kind.into(),
        since: 1,
    };
    Manifest {
        generation: 1,
        dver: 1,
        min_dver: 1,
        fields: vec![
            field("high_bid", "integer"),
            field("owner", "string"),
            field("status", "string"),
            field("title", "string"),
        ],
        ..Default::default()
    }
}

fn measure(n: u64) {
    let dir = tempfile::tempdir().expect("tempdir");
    let scratch = dir.path();
    let rows: Vec<DeltaRow> = (0..n).map(|i| row(i, 1)).collect();
    let t = Instant::now();
    let base = delta::encode(&rows, None, scratch).expect("encodes");
    let encode_ms = t.elapsed().as_millis();
    let base_mb = base.len() as f64 / 1e6;
    // The fold: one small delta (100 rewritten rows) over the whole base.
    let small: Vec<DeltaRow> = (0..100).map(|i| row(i * 97 % n, 2)).collect();
    let small_bytes = delta::encode(&small, None, scratch).expect("encodes");
    let m = manifest();
    let t = Instant::now();
    let merged = compact::merge(Some(&base), std::slice::from_ref(&small_bytes), &m, scratch)
        .expect("merges");
    let fold_ms = t.elapsed().as_millis();
    // The read side: a node materializing the class from base + one delta.
    let path = scratch.join("overlay.sqlite");
    let t = Instant::now();
    let overlay = Overlay::build(
        Some(&base),
        std::slice::from_ref(&small_bytes),
        &m,
        &path,
        scratch,
    )
    .expect("builds");
    let build_ms = t.elapsed().as_millis();
    let overlay_mb = std::fs::metadata(&path).map(|f| f.len()).unwrap_or(0) as f64 / 1e6;
    let query = || Query {
        where_: Where(vec![Condition::Compare {
            field: "status".into(),
            op: Compare::Eq,
            value: Value::Text("open".into()),
        }]),
        order: vec![Order {
            field: "high_bid".into(),
            descending: true,
        }],
        limit: 50,
        cursor: None,
    };
    // The hot path: one more delta applied in place, what a flush now
    // costs a reader instead of a rebuild.
    let later: Vec<DeltaRow> = (0..100).map(|i| row(i * 89 % n, 3)).collect();
    let later_bytes = delta::encode(&later, None, scratch).expect("encodes");
    let t = Instant::now();
    overlay
        .apply(std::slice::from_ref(&later_bytes), scratch)
        .expect("applies");
    let apply_ms = t.elapsed().as_millis();
    let t = Instant::now();
    let page = overlay.list(&query(), &m).expect("lists");
    let query_ms = t.elapsed().as_micros() as f64 / 1000.0;
    let t = Instant::now();
    for _ in 0..20 {
        overlay.list(&query(), &m).expect("lists");
    }
    let query_warm_ms = t.elapsed().as_micros() as f64 / 1000.0 / 20.0;
    println!(
        "rows={n:>8}  base={base_mb:6.1}MB encode={encode_ms:>6}ms | FOLD(100-row delta)={fold_ms:>6}ms merged={:.1}MB | OVERLAY build={build_ms:>6}ms file={overlay_mb:.1}MB apply(100 rows)={apply_ms:>5}ms | query first={query_ms:.1}ms warm={query_warm_ms:.2}ms entries={}",
        merged.bytes.len() as f64 / 1e6,
        page.entries.len()
    );
}

#[test]
#[ignore]
fn the_directory_at_size() {
    for n in [10_000u64, 100_000, 1_000_000] {
        measure(n);
    }
}
