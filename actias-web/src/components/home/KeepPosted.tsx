import * as React from 'react';
import { useMutation } from '@tanstack/react-query';
import api from '@/helpers/api';
import { HomeIcon } from '@/components/home/HomeIcon';
import classes from './KeepPosted.module.css';

/** The landing's one form. Nothing is sent from the list yet, so the
 * confirmation says that rather than promising mail. */
export function KeepPosted() {
  const [email, setEmail] = React.useState('');

  const signUp = useMutation({
    mutationFn: (address: string) =>
      api.interest.keepMePosted({ email: address, source: 'landing' }),
  });

  const failure =
    signUp.error &&
    ((signUp.error as { body?: { message?: string } }).body?.message ||
      'That did not go through. Try again in a moment.');

  if (signUp.isSuccess) {
    return (
      <div className={classes.done}>
        <span className={classes.tick}>
          <HomeIcon name="check" size={15} />
        </span>
        <div>
          <p className={classes.doneTitle}>{email} is on the list.</p>
          <p className={classes.doneBody}>
            Nothing is sent from it yet. It is how the first announcement finds
            you when there is one.
          </p>
        </div>
      </div>
    );
  }

  return (
    <form
      className={classes.form}
      onSubmit={(event) => {
        event.preventDefault();
        const address = email.trim();
        if (address) signUp.mutate(address);
      }}
    >
      <div className={classes.row}>
        <input
          type="email"
          required
          value={email}
          onChange={(event) => setEmail(event.target.value)}
          placeholder="you@example.com"
          aria-label="Email address"
          aria-invalid={failure ? true : undefined}
          className={classes.input}
          disabled={signUp.isPending}
        />
        <button
          type="submit"
          className={classes.submit}
          disabled={signUp.isPending}
        >
          {signUp.isPending ? 'Sending' : 'Keep me posted'}
          <HomeIcon name="arrowRight" size={14} />
        </button>
      </div>
      {failure && (
        <p className={classes.error} role="alert">
          {failure}
        </p>
      )}
    </form>
  );
}
