import * as React from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import api, { showError } from '@/helpers/api';
import { toast } from '@/ui/toast';
import { AdminFrame } from '@/components/admin/AdminFrame';
import { Field } from '@/ui';
import classes from '@/components/inspector.module.css';

interface RegistrationCode {
  id: string;
  uses: number;
  createdAt: string;
}

const COLUMNS = 'minmax(0,1fr) 110px 110px 190px';

/** The register link a code rides in, on this instance's own origin. */
function inviteLink(code: string) {
  return `${window.location.origin}/register?code=${code}`;
}

export default function AdminInvites() {
  const queryClient = useQueryClient();

  const { data: settings } = useQuery({
    queryKey: ['registration-settings'],
    queryFn: () => api.admin.registrationSettings(),
  });
  const { data: codes } = useQuery({
    queryKey: ['registration-codes'],
    queryFn: async () =>
      (
        (await api.admin.listRegistrationCodes(1)) as unknown as {
          items: RegistrationCode[];
        }
      ).items,
  });

  const reload = () =>
    queryClient.invalidateQueries({ queryKey: ['registration-codes'] });

  const sendInvite = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    const email = String(new FormData(form).get('email') ?? '');
    api.admin
      .createInvite({ email })
      .then((result) => {
        if (result.emailed) {
          toast({ title: 'Invite sent', message: email });
        } else {
          navigator.clipboard.writeText(result.link);
          toast({
            title: 'Link copied',
            message: 'SMTP is off, so send it yourself.',
          });
        }
        form.reset();
        reload();
      })
      .catch(showError);
  };

  const createLink = () => {
    api.admin
      .newRegistrationCode(1)
      .then((code) => {
        navigator.clipboard.writeText(inviteLink(code.id));
        toast({ title: 'Link copied', message: 'One use.' });
        reload();
      })
      .catch(showError);
  };

  const copyLink = (code: string) => {
    navigator.clipboard.writeText(inviteLink(code));
    toast({ title: 'Link copied', message: 'Paste it to whoever it is for.' });
  };

  const revoke = (code: string) => {
    api.admin
      .deleteRegistrationCode(code)
      .then(() => {
        toast({ title: 'Invite revoked', message: code.slice(0, 8) });
        reload();
      })
      .catch(showError);
  };

  return (
    <AdminFrame
      title="Invites"
      hint={
        settings?.inviteOnly
          ? 'Registration is invite-only: joining this instance takes a link from here.'
          : 'Registration is open; invites are optional on this instance.'
      }
      actions={
        <button className={classes.accentButton} onClick={createLink}>
          New link
        </button>
      }
    >
      {settings?.smtpEnabled && (
        <div className={classes.card} style={{ padding: '4px 14px 14px' }}>
          <form
            onSubmit={sendInvite}
            style={{ display: 'flex', gap: 8, alignItems: 'end' }}
          >
            <div style={{ flex: 1 }}>
              <Field
                label="Invite by email"
                name="email"
                type="email"
                placeholder="them@example.com"
                required
              />
            </div>
            <button className={classes.accentButton} type="submit">
              Send invite
            </button>
          </form>
        </div>
      )}

      <div className={classes.card}>
        <div
          className={classes.tableHead}
          style={{ gridTemplateColumns: COLUMNS, position: 'static' }}
        >
          <span>code</span>
          <span>uses left</span>
          <span>created</span>
          <span />
        </div>
        {(codes ?? []).length === 0 && (
          <p style={{ color: 'var(--ink-3)', fontSize: 12, padding: 14 }}>
            No open invites. A new link admits one person; revoke it and it
            admits nobody.
          </p>
        )}
        {(codes ?? []).map((code: RegistrationCode) => (
          <div
            key={code.id}
            className={classes.row}
            style={{ gridTemplateColumns: COLUMNS }}
          >
            <span className={classes.cellMono}>{code.id}</span>
            <span className={classes.cellDim}>{code.uses}</span>
            <span className={classes.cellDim}>
              {new Date(code.createdAt).toLocaleDateString()}
            </span>
            <span style={{ display: 'flex', gap: 6, justifyContent: 'end' }}>
              <button
                className={classes.ghostButton}
                onClick={() => copyLink(code.id)}
              >
                Copy link
              </button>
              <button
                className={classes.ghostButton}
                onClick={() => revoke(code.id)}
              >
                Revoke
              </button>
            </span>
          </div>
        ))}
      </div>
    </AdminFrame>
  );
}
