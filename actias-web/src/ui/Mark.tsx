/**
 * The Actias mark: the luna moth as a single-weight line glyph, no fill,
 * straight from the foundations design document. Below 20px the antennae
 * drop; same drawing, one variant.
 */
export function Mark({ size = 20 }: { size?: number }) {
  const antennae = size >= 20;
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="var(--luna)"
      strokeWidth={antennae ? 1.5 : 1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-label="Actias"
    >
      <path d="M12 7.2 L12 14.4" />
      {antennae && <path d="M12 7 C 11 5.4 10.1 4.7 8.8 4.3" />}
      {antennae && <path d="M12 7 C 13 5.4 13.9 4.7 15.2 4.3" />}
      <path d="M12 7.6 C 7.6 5.4 3.1 7.4 3.1 10.6 C 3.1 12.9 6.5 13.9 9.6 13.3" />
      <path d="M12 7.6 C 16.4 5.4 20.9 7.4 20.9 10.6 C 20.9 12.9 17.5 13.9 14.4 13.3" />
      <path d="M9.7 13.4 C 8.2 16.8 8.4 20.1 10.2 21.9 C 10.9 18.6 11.3 16.1 11.9 13.9" />
      <path d="M14.3 13.4 C 15.8 16.8 15.6 20.1 13.8 21.9 C 13.1 18.6 12.7 16.1 12.1 13.9" />
    </svg>
  );
}
