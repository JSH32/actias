/**
 * The project's runtime policy: what its scripts may spend on a node
 * and where they may reach. Two rates and two host lists, read by every
 * worker within a pointer's ttl; readable with the scripts bit, edited
 * with full access. A rate of 0 is the platform default (unbounded); an
 * empty allow list admits every host the deny list and the node's own
 * policy do not refuse.
 *
 * Two cards, one per concern. A rate is a choice first (no limit, or a
 * limit) and a number second, so the stored 0 never shows. Hosts are
 * edited as rows or as text over one draft, with the line the api would
 * refuse marked before the save is attempted.
 */
import * as React from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  AlignLeft,
  Check,
  Gauge,
  Globe,
  List,
  Lock,
  Plus,
  X,
} from 'lucide-react';
import api, { showError } from '@/helpers/api';
import {
  ProjectDto,
  ProjectMoveDto,
  ProjectPolicyDto,
  ProjectPolicyViewDto,
} from '@/client';
import ProjectSection from '@/components/ProjectSection';
import { toast } from '@/ui/toast';
import classes from '../../../components/inspector.module.css';

/** One host per line; blank lines dropped, case and spaces folded the
 * way the api stores them. */
function hostsFrom(text: string): string[] {
  return text
    .split('\n')
    .map((line) => line.trim().toLowerCase())
    .filter((line) => line.length > 0);
}

/** The api's rule: a scheme, a path or a port is not a host name. */
function isHost(entry: string): boolean {
  return entry.length > 0 && !/[/ :]/.test(entry);
}

/** The first line the api would refuse, one-based, so the message can
 * point at it. */
function badHostLine(text: string): { line: number; text: string } | null {
  const lines = text.split('\n');
  for (let i = 0; i < lines.length; i += 1) {
    const host = lines[i].trim();
    if (host.length > 0 && !isHost(host)) {
      return { line: i + 1, text: host };
    }
  }
  return null;
}

function rateFrom(text: string): number {
  const parsed = Number.parseInt(text, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 0;
}

function plural(count: number, noun: string) {
  return `${count} ${noun}${count === 1 ? '' : 's'}`;
}

const ICON = 14;

const inputStyle: React.CSSProperties = {
  height: 32,
  padding: '0 10px',
  border: '1px solid var(--line)',
  borderRadius: 'var(--r2)',
  font: '400 13px var(--mono)',
  boxSizing: 'border-box',
};

const textareaStyle: React.CSSProperties = {
  minHeight: 132,
  width: '100%',
  padding: '8px 10px',
  border: '1px solid var(--line)',
  borderRadius: 'var(--r2)',
  font: '400 12px/1.7 var(--mono)',
  resize: 'vertical',
  boxSizing: 'border-box',
};

const hintStyle = (bad: boolean): React.CSSProperties => ({
  fontSize: 11,
  lineHeight: 1.5,
  color: bad ? 'var(--err)' : 'var(--ink-3)',
});

/** A two-way switch: the chosen half is lit, each half may carry a
 * glyph. Sits beside the thing it switches, never at the far edge. */
function Switch<T extends string>({
  options,
  chosen,
  write,
  onChoose,
}: {
  options: { value: T; label: string; icon?: React.ReactNode }[];
  chosen: T;
  write: boolean;
  onChoose: (next: T) => void;
}) {
  return (
    <div
      role="radiogroup"
      style={{
        display: 'inline-flex',
        border: '1px solid var(--line)',
        borderRadius: 'var(--r2)',
        overflow: 'hidden',
        flexShrink: 0,
      }}
    >
      {options.map((option, index) => {
        const active = chosen === option.value;
        return (
          <button
            key={option.value}
            type="button"
            role="radio"
            aria-checked={active}
            disabled={!write}
            onClick={() => onChoose(option.value)}
            style={{
              display: 'inline-flex',
              alignItems: 'center',
              gap: 6,
              height: 28,
              padding: '0 10px',
              border: 'none',
              borderLeft: index > 0 ? '1px solid var(--line)' : 'none',
              background: active ? 'var(--night-2)' : 'transparent',
              color: active ? 'var(--ink-1)' : 'var(--ink-3)',
              font: '400 12px var(--mono)',
              cursor: write ? 'pointer' : 'default',
            }}
          >
            {option.icon}
            {option.label}
          </button>
        );
      })}
    </div>
  );
}

function Card({
  icon,
  label,
  lede,
  children,
}: {
  icon: React.ReactNode;
  label: string;
  lede: string;
  children: React.ReactNode;
}) {
  return (
    <div className={classes.card}>
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          gap: 4,
          padding: '14px 18px 12px',
          borderBottom: '1px solid var(--line)',
        }}
      >
        <span
          className={classes.sectionLabel}
          style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}
        >
          {icon}
          {label}
        </span>
        <span style={{ fontSize: 12, color: 'var(--ink-2)' }}>{lede}</span>
      </div>
      {children}
    </div>
  );
}

/** A rate is a choice first and a number second. The platform stores
 * "no limit" as 0, which the form never shows; a limit chosen without
 * a number blocks the save rather than saving as 0. */
type RateDraft = { limited: boolean; value: string };

function rateDraft(stored: number): RateDraft {
  return { limited: stored > 0, value: stored > 0 ? String(stored) : '' };
}

function rateOf(draft: RateDraft): number {
  return draft.limited ? rateFrom(draft.value) : 0;
}

function rateIncomplete(draft: RateDraft): boolean {
  return draft.limited && rateFrom(draft.value) === 0;
}

function Rate({
  label,
  unit,
  whenLimited,
  whenOpen,
  draft,
  write,
  onChange,
}: {
  label: string;
  unit: string;
  whenLimited: string;
  whenOpen: string;
  draft: RateDraft;
  write: boolean;
  onChange: (next: RateDraft) => void;
}) {
  const incomplete = rateIncomplete(draft);
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 12,
          flexWrap: 'wrap',
        }}
      >
        <span style={{ fontSize: 13, color: 'var(--ink-1)', width: 96 }}>
          {label}
        </span>
        <Switch
          options={[
            { value: 'open', label: 'No limit' },
            { value: 'limit', label: 'Limit' },
          ]}
          chosen={draft.limited ? 'limit' : 'open'}
          write={write}
          onChoose={(next) => onChange({ ...draft, limited: next === 'limit' })}
        />
        {draft.limited && (
          <span
            style={{ display: 'inline-flex', alignItems: 'center', gap: 8 }}
          >
            <input
              className={classes.searchInput}
              style={{
                ...inputStyle,
                width: 120,
                borderColor: incomplete ? 'var(--err)' : 'var(--line)',
              }}
              type="text"
              inputMode="numeric"
              pattern="[0-9]*"
              placeholder="100"
              autoFocus={draft.value === ''}
              value={draft.value}
              disabled={!write}
              onChange={(event) =>
                onChange({
                  ...draft,
                  value: event.target.value.replace(/[^0-9]/g, ''),
                })
              }
            />
            <span style={{ fontSize: 12, color: 'var(--ink-2)' }}>{unit}</span>
          </span>
        )}
      </div>
      <span style={{ ...hintStyle(incomplete), paddingLeft: 108 }}>
        {incomplete
          ? 'Enter a number above 0, or choose no limit.'
          : draft.limited
          ? whenLimited
          : whenOpen}
      </span>
    </div>
  );
}

/** A host list, edited as rows or as text. The draft underneath is one
 * string, one host per line, so the two views never disagree and the
 * text view keeps whatever a paste brought in. */
function Hosts({
  label,
  hint,
  placeholder,
  value,
  write,
  onChange,
}: {
  label: string;
  hint: string;
  placeholder: string;
  value: string;
  write: boolean;
  onChange: (next: string) => void;
}) {
  const [mode, setMode] = React.useState<'list' | 'text'>('list');
  const [pending, setPending] = React.useState('');
  const hosts = hostsFrom(value);
  const bad = badHostLine(value);
  const pendingHost = pending.trim().toLowerCase();
  const pendingBad = pendingHost.length > 0 && !isHost(pendingHost);

  const add = () => {
    if (pendingHost.length === 0 || pendingBad) return;
    if (!hosts.includes(pendingHost)) {
      onChange([...hosts, pendingHost].join('\n'));
    }
    setPending('');
  };
  const remove = (host: string) => {
    onChange(hosts.filter((entry) => entry !== host).join('\n'));
  };

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 10,
          flexWrap: 'wrap',
        }}
      >
        <span style={{ fontSize: 13, color: 'var(--ink-1)' }}>{label}</span>
        {hosts.length > 0 && (
          <span className={classes.wordChip}>
            {plural(hosts.length, 'host')}
          </span>
        )}
        <Switch
          options={[
            { value: 'list', label: 'List', icon: <List size={ICON} /> },
            { value: 'text', label: 'Text', icon: <AlignLeft size={ICON} /> },
          ]}
          chosen={mode}
          write
          onChoose={setMode}
        />
      </div>

      {mode === 'list' ? (
        <div
          style={{
            border: `1px solid ${bad ? 'var(--err)' : 'var(--line)'}`,
            borderRadius: 'var(--r2)',
            overflow: 'hidden',
          }}
        >
          {hosts.length === 0 ? (
            <div
              style={{
                padding: '12px 10px',
                font: '400 12px var(--mono)',
                color: 'var(--ink-3)',
              }}
            >
              No hosts.
            </div>
          ) : (
            hosts.map((host) => {
              const invalid = !isHost(host);
              return (
                <div
                  key={host}
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'space-between',
                    gap: 8,
                    padding: '6px 6px 6px 10px',
                    borderBottom: '1px solid var(--line)',
                    font: '400 12px var(--mono)',
                    color: invalid ? 'var(--err)' : 'var(--ink-1)',
                  }}
                >
                  <span style={{ wordBreak: 'break-all' }}>{host}</span>
                  {write && (
                    <button
                      type="button"
                      className={classes.smallButton}
                      style={{
                        display: 'inline-flex',
                        alignItems: 'center',
                        padding: '0 5px',
                      }}
                      aria-label={`Remove ${host}`}
                      title="Remove"
                      onClick={() => remove(host)}
                    >
                      <X size={12} />
                    </button>
                  )}
                </div>
              );
            })
          )}
          {write && (
            <div
              style={{
                display: 'flex',
                gap: 6,
                padding: 6,
                background: 'var(--night-2)',
              }}
            >
              <input
                className={classes.searchInput}
                style={{
                  ...inputStyle,
                  flex: 1,
                  height: 30,
                  font: '400 12px var(--mono)',
                  borderColor: pendingBad ? 'var(--err)' : 'var(--line)',
                }}
                placeholder={placeholder.split('\n')[0]}
                spellCheck={false}
                value={pending}
                onChange={(event) => setPending(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') {
                    event.preventDefault();
                    add();
                  }
                }}
              />
              <button
                type="button"
                className={classes.ghostButton}
                style={{
                  display: 'inline-flex',
                  alignItems: 'center',
                  justifyContent: 'center',
                  width: 30,
                  height: 30,
                  padding: 0,
                }}
                aria-label="Add host"
                title="Add"
                disabled={pendingHost.length === 0 || pendingBad}
                onClick={add}
              >
                <Plus size={ICON} />
              </button>
            </div>
          )}
        </div>
      ) : (
        <textarea
          className={classes.searchInput}
          style={{
            ...textareaStyle,
            borderColor: bad ? 'var(--err)' : 'var(--line)',
          }}
          placeholder={placeholder}
          spellCheck={false}
          value={value}
          disabled={!write}
          onChange={(event) => onChange(event.target.value)}
        />
      )}

      <span style={hintStyle(bad !== null || pendingBad)}>
        {pendingBad
          ? 'Not a host name. Hosts carry no scheme, path or port.'
          : bad
          ? `Line ${bad.line} is not a host name: "${bad.text}". Hosts carry no scheme, path or port.`
          : hint}
      </span>
    </div>
  );
}

/** The editable half of a policy read: what the form drafts and saves. */
function editable(policy: ProjectPolicyViewDto): ProjectPolicyDto {
  return {
    requestsPerSec: policy.requestsPerSec,
    workUnitsPerSec: policy.workUnitsPerSec,
    egressAllow: policy.egressAllow,
    egressDeny: policy.egressDeny,
  };
}

function PolicyForm({
  project,
  policy,
  write,
}: {
  project: ProjectDto;
  policy: ProjectPolicyViewDto;
  write: boolean;
}) {
  const queryClient = useQueryClient();
  const [requests, setRequests] = React.useState(() =>
    rateDraft(policy.requestsPerSec),
  );
  const [work, setWork] = React.useState(() =>
    rateDraft(policy.workUnitsPerSec),
  );
  const [allow, setAllow] = React.useState(policy.egressAllow.join('\n'));
  const [deny, setDeny] = React.useState(policy.egressDeny.join('\n'));

  const drafted: ProjectPolicyDto = {
    requestsPerSec: rateOf(requests),
    workUnitsPerSec: rateOf(work),
    egressAllow: hostsFrom(allow),
    egressDeny: hostsFrom(deny),
  };
  const dirty = JSON.stringify(drafted) !== JSON.stringify(editable(policy));
  const invalid =
    badHostLine(allow) !== null ||
    badHostLine(deny) !== null ||
    rateIncomplete(requests) ||
    rateIncomplete(work);

  const save = useMutation({
    mutationFn: () => api.project.setPolicy(project.id, drafted),
    onSuccess: (saved: ProjectPolicyViewDto) => {
      queryClient.setQueryData(['policy', project.id], saved);
      toast({
        title: 'Policy saved',
        message: 'Workers pick it up within a minute.',
      });
    },
    onError: showError,
  });

  const reset = () => {
    setRequests(rateDraft(policy.requestsPerSec));
    setWork(rateDraft(policy.workUnitsPerSec));
    setAllow(policy.egressAllow.join('\n'));
    setDeny(policy.egressDeny.join('\n'));
  };

  const status = invalid
    ? 'Fix the marked field before saving.'
    : dirty
    ? 'Unsaved changes.'
    : 'Saved. Workers pick a change up within a minute.';

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
      <Card
        icon={<Gauge size={13} />}
        label="Rates"
        lede="What a node lets this project spend per second. Its fair share of the node holds either way."
      >
        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            gap: 16,
            padding: '16px 18px',
          }}
        >
          <Rate
            label="Requests"
            unit="per second"
            whenLimited="Admitted per node with a burst of the same size; over it a request answers 429 with Retry-After."
            whenOpen="Every request is admitted; only the node's own limits and the fair share hold."
            draft={requests}
            write={write}
            onChange={setRequests}
          />
          <Rate
            label="Work units"
            unit="per second"
            whenLimited="Charged after each call, so a debt refuses until it refills."
            whenOpen="Calls spend what they need; only the node's own limits and the fair share hold."
            draft={work}
            write={write}
            onChange={setWork}
          />
        </div>
      </Card>

      <Card
        icon={<Globe size={13} />}
        label="Egress"
        lede="Where outbound requests and dials from this project may reach. A host is refused if the deny list matches it, then admitted if the allow list is empty or matches it, then held to the node's own policy."
      >
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: 'repeat(auto-fit, minmax(340px, 1fr))',
            gap: 18,
            padding: '16px 18px',
          }}
        >
          <Hosts
            label="Allowed hosts"
            hint="A leading dot matches subdomains. Empty admits everything not denied."
            placeholder={'api.example.com\n.internal.example.com'}
            value={allow}
            write={write}
            onChange={setAllow}
          />
          <Hosts
            label="Denied hosts"
            hint="Refused before the allow list is consulted."
            placeholder="metadata.google.internal"
            value={deny}
            write={write}
            onChange={setDeny}
          />
        </div>
      </Card>

      {write ? (
        <div
          className={classes.card}
          style={{
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
            gap: 12,
            padding: '10px 12px 10px 18px',
          }}
        >
          <span
            style={{
              font: '400 11px var(--mono)',
              color: invalid ? 'var(--err)' : 'var(--ink-3)',
            }}
          >
            {status}
          </span>
          <div style={{ display: 'flex', gap: 8 }}>
            <button
              className={classes.ghostButton}
              onClick={reset}
              disabled={!dirty || save.isPending}
            >
              Discard
            </button>
            <button
              className={classes.accentButton}
              style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}
              onClick={() => save.mutate()}
              disabled={!dirty || invalid || save.isPending}
            >
              <Check size={ICON} />
              {save.isPending ? 'Saving…' : 'Save policy'}
            </button>
          </div>
        </div>
      ) : (
        <span
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            gap: 6,
            font: '400 11px var(--mono)',
            color: 'var(--ink-3)',
          }}
        >
          <Lock size={12} />
          Full access on the project edits this policy.
        </span>
      )}
    </div>
  );
}

/** One line on the latest move: where from and to, the step it is at,
 * the objects copied, the error if it failed. */
function MoveLine({ move }: { move: ProjectMoveDto }) {
  const running = move.step !== 'done' && move.step !== 'failed';
  const text =
    move.step === 'done'
      ? `Moved from ${move.fromRegion} to ${move.toRegion}.`
      : move.step === 'failed'
      ? `Move from ${move.fromRegion} to ${move.toRegion} failed: ${move.error}`
      : `Moving from ${move.fromRegion} to ${move.toRegion}: ${move.step}` +
        (move.step === 'copying'
          ? `, ${move.objectsCopied} of ${move.objectsTotal} objects`
          : '') +
        '.';
  return (
    <span
      style={{
        font: '400 11px var(--mono)',
        color:
          move.step === 'failed'
            ? 'var(--err)'
            : running
            ? 'var(--warn)'
            : 'var(--ink-3)',
      }}
    >
      {text}
    </span>
  );
}

function Policy({ project, write }: { project: ProjectDto; write: boolean }) {
  const { data: policy, error } = useQuery({
    queryKey: ['policy', project.id],
    queryFn: () => api.project.getPolicy(project.id),
    // A move flips the home and clears the mark; the page follows.
    refetchInterval: (query) => (query.state.data?.moving ? 3_000 : false),
  });
  const { data: move } = useQuery({
    queryKey: ['move', project.id],
    queryFn: () => api.project.getMove(project.id),
    refetchInterval: (query) => {
      const step = query.state.data?.step;
      return step && step !== 'done' && step !== 'failed' ? 3_000 : false;
    },
  });
  const failure = error as { body?: { message?: string } } | null;

  return (
    <div className={classes.frame}>
      <div className={classes.frameHeadPadded}>
        <div className={classes.headTop}>
          <div className={classes.headMain}>
            <div className={classes.pageHead}>
              <h1 className={classes.pageTitle}>Policy</h1>
              {policy && (
                <span className={classes.metaChip}>
                  home <strong>{policy.region}</strong>
                </span>
              )}
              {policy?.moving && (
                <span className={classes.metaChip}>moving</span>
              )}
            </div>
            {move && move.step !== '' && <MoveLine move={move} />}
            <p className={classes.lede}>
              What this project&apos;s scripts may spend on a node and where
              they may reach. Every worker applies it; a node&apos;s own limits
              and its fair share still hold above it.
            </p>
          </div>
        </div>
      </div>

      <div style={{ flex: 1, overflowY: 'auto', minHeight: 0 }}>
        <div style={{ padding: '18px 20px', maxWidth: 1180 }}>
          {failure ? (
            <p style={{ color: 'var(--ink-2)', margin: 0 }}>
              The policy could not be loaded:{' '}
              {failure.body?.message ?? 'the request failed'}.
            </p>
          ) : !policy ? (
            <p style={{ color: 'var(--ink-3)', margin: 0 }}>Loading…</p>
          ) : (
            <PolicyForm
              // Remount on a fresh read so the draft starts from it.
              key={JSON.stringify(policy)}
              project={project}
              policy={policy}
              write={write}
            />
          )}
        </div>
      </div>
    </div>
  );
}

export default function PolicyPage() {
  return (
    <ProjectSection
      permission="SCRIPT_READ"
      writeBit="FULL"
      render={(project, write) => <Policy project={project} write={write} />}
    />
  );
}
