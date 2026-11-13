/**
 * The PCHI mark: the peach from pchi.life, with the "pchi." wordmark and
 * "Studio" beside it. The stroke follows the text colour, so it works on any
 * background; the fills are the brand's own peach and leaf.
 */
export default function Logo() {
  return (
    <span className="logo" role="img" aria-label="PCHI Studio">
      <svg className="logo-peach" width="24" height="25" viewBox="0 0 40 40" aria-hidden="true">
        <path d="M21 11C15 5 5 11 5 21c0 9 9 15 15 15s15-7 15-16c0-10-9-14-14-9Z" fill="#ed9972" stroke="currentColor" strokeWidth="1.6" />
        <path d="M20 11c4 5 4 15 0 22M21 10c0-4 3-7 5-8" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
        <path d="M20 10C13 11 8 6 9 3c6-1 10 2 11 7Z" fill="#869365" stroke="currentColor" strokeWidth="1.4" />
      </svg>
      <span className="logo-word" aria-hidden="true">
        pchi<span className="logo-dot">.</span>
      </span>
      <span className="logo-sub" aria-hidden="true">Studio</span>
    </span>
  )
}
