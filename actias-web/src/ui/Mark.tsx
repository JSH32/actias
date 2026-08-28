/**
 * The Actias mark: a luna moth in flight posture, forewings swept up and
 * out, hindwings drawn down into the trailing streamers the species is
 * known for, all hung on a body.
 *
 * The body is the load-bearing part. The previous drawing was four lobes
 * radiating from a centre of negative space, which is how a palmate leaf
 * is built rather than an insect, and in luna green it read as cannabis
 * before it read as a moth. Wings meeting a body instead of each other
 * settles that, and the top-heavy mass (broad forewings, narrow tails)
 * is a silhouette no leaf has.
 */
export function Mark({
  size = 20,
  className,
}: {
  size?: number;
  className?: string;
}) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="var(--luna)"
      className={className}
      aria-label="Actias"
    >
      <path
        data-wing="fore-l"
        d="M11.2 12.4C9.8 8.2 7 5.4 3.6 4.4 3.6 9.2 6.4 12.2 11.2 12.4Z"
      />
      <path
        data-wing="fore-r"
        d="M12.8 12.4C14.2 8.2 17 5.4 20.4 4.4 20.4 9.2 17.6 12.2 12.8 12.4Z"
      />
      <path
        data-wing="hind-l"
        d="M11.3 13.4C10.4 16.2 8.8 18.4 6.2 20.8 7 17.4 8.8 15 11.6 13.6Z"
      />
      <path
        data-wing="hind-r"
        d="M12.7 13.4C13.6 16.2 15.2 18.4 17.8 20.8 17 17.4 15.2 15 12.4 13.6Z"
      />
      <path
        data-wing="body"
        d="M12 3.6c.7 0 1.1.8 1.05 2l-.2 7.4c-.05 2.6-.35 4.6-.85 6.6-.5-2-.8-4-.85-6.6l-.2-7.4c-.05-1.2.35-2 1.05-2Z"
      />
    </svg>
  );
}

/**
 * The alternative kept from the same exploration: the retired four-lobe
 * drawing with a body threaded through it, head and antennae above the
 * forewings, abdomen below the hindwings. It escapes the leaf reading by
 * the same means [`Mark`] does while staying closer to what shipped
 * before, and it has a third pair of parts to lag in motion, which the
 * flight posture does not.
 *
 * Unused today and kept deliberately, not by accident: the choice
 * between the two is recorded as taste rather than correctness, so the
 * losing drawing stays reachable instead of living in a screenshot. Its
 * antennae are the only stroked geometry in the set and clog below
 * roughly 20 px, so a favicon built from this one drops them.
 *
 * The `data-wing` names match [`Mark`], so the brand hover animates this
 * drawing too; its wing roots sit about a unit higher, which the shared
 * pivots absorb at the sizes the mark is used at.
 */
export function SpineMark({
  size = 20,
  className,
}: {
  size?: number;
  className?: string;
}) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="var(--luna)"
      className={className}
      aria-label="Actias"
    >
      <path
        data-wing="fore-l"
        d="M11 11.6C10.3 7.8 8 5.4 4.8 5 4.6 8.6 6.6 11 11 11.6Z"
      />
      <path
        data-wing="fore-r"
        d="M13 11.6C13.7 7.8 16 5.4 19.2 5 19.4 8.6 17.4 11 13 11.6Z"
      />
      <path
        data-wing="hind-l"
        d="M11 12.8C6.8 13.4 4.8 15.8 5 19.2 8.2 18.6 10.4 16.4 11 12.8Z"
      />
      <path
        data-wing="hind-r"
        d="M13 12.8C17.2 13.4 19.2 15.8 19 19.2 15.8 18.6 13.6 16.4 13 12.8Z"
      />
      <path
        data-wing="body"
        d="M12 2.6c.75 0 1.2.9 1.15 2.2l-.2 8.4c-.05 3.2-.4 5.8-.95 8.4-.55-2.6-.9-5.2-.95-8.4l-.2-8.4C10.8 3.5 11.25 2.6 12 2.6Z"
      />
      {size >= 20 && (
        <>
          <path
            data-wing="ant-l"
            d="M11.4 3.2C10.2 2.2 9 1.9 7.9 1.9"
            fill="none"
            stroke="var(--luna)"
            strokeWidth="1.1"
            strokeLinecap="round"
          />
          <path
            data-wing="ant-r"
            d="M12.6 3.2C13.8 2.2 15 1.9 16.1 1.9"
            fill="none"
            stroke="var(--luna)"
            strokeWidth="1.1"
            strokeLinecap="round"
          />
        </>
      )}
    </svg>
  );
}
