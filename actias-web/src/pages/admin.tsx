import * as React from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import api, { showError } from '@/helpers/api';
import { AuthGuard } from '@/helpers/auth';
import { Button, Card, Field } from '@/ui';
import { toast } from '@/ui/toast';
import classes from './projects.module.css';

interface RegistrationCode {
  id: string;
  uses: number;
}

function Admin() {
  const queryClient = useQueryClient();

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

  const createCode = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const uses = Number(new FormData(event.currentTarget).get('uses') ?? 1);
    api.admin
      .newRegistrationCode(uses)
      .then(() => {
        toast({ title: 'Code created', message: `${uses} use(s)` });
        reload();
      })
      .catch(showError);
  };

  const deleteCode = (code: string) => {
    api.admin
      .deleteRegistrationCode(code)
      .then(() => {
        toast({ title: 'Code deleted', message: code });
        reload();
      })
      .catch(showError);
  };

  return (
    <div style={{ maxWidth: 560 }}>
      <h1 style={{ fontSize: 18, fontWeight: 700, marginBottom: 12 }}>
        Registration codes
      </h1>
      <Card style={{ padding: 16, marginBottom: 12 }}>
        <form onSubmit={createCode} style={{ display: 'flex', gap: 8 }}>
          <div style={{ flex: 1 }}>
            <Field
              label="Uses"
              name="uses"
              type="number"
              defaultValue={1}
              min={1}
              required
            />
          </div>
          <div style={{ alignSelf: 'flex-end' }}>
            <Button type="submit" variant="primary">
              New code
            </Button>
          </div>
        </form>
      </Card>
      <Card>
        <table className={classes.table}>
          <thead>
            <tr>
              <th>code</th>
              <th>uses left</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {(codes ?? []).map((code: RegistrationCode) => (
              <tr key={code.id}>
                <td
                  className={classes.name}
                  style={{ fontFamily: 'var(--mono)', fontSize: 12 }}
                >
                  {code.id}
                </td>
                <td className={classes.meta}>{code.uses}</td>
                <td style={{ textAlign: 'right' }}>
                  <Button variant="danger" onClick={() => deleteCode(code.id)}>
                    Delete
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </Card>
    </div>
  );
}

export default function AdminPage() {
  return (
    <AuthGuard>
      <Admin />
    </AuthGuard>
  );
}
