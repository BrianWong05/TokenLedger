// The app's mark: design 4b's sparkline with fill — the same drawing as the app
// icon (src-tauri/icons/icon.svg), without its tile. One component so the
// sidebar and the first-run badge cannot drift apart, and currentColor so each
// decides its own: the accent on the sidebar, white on the badge.
//
// Two departures from the icon, both because this renders at 18-20px rather
// than 512. The endpoint is a stroked ring, not a disc with the tile punched
// out, so its hole is actually transparent and the mark sits on any background;
// and the line stops short of that ring, since at this size its round cap would
// fill the hole it is meant to leave.
export function Mark({ size = 18 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 20 20" fill="none" aria-hidden="true">
      <path
        d="M2.1 16.3 L5.05 12.3 L7 14.4 L14.35 6.25 L16 8.3 L16 19 L2.1 19 Z"
        fill="currentColor"
        fillOpacity="0.24"
      />
      <path
        d="M2.1 16.3 L5.05 12.3 L7 14.4 L14.35 6.25"
        stroke="currentColor"
        strokeWidth="2.1"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <circle cx="16" cy="4.4" r="2.05" stroke="currentColor" strokeWidth="1.8" />
    </svg>
  );
}
