/* Drawn marks. One 1.6 stroke, 16px box, square caps — panel silkscreen, not emoji. */

const box = (body: string, size = 16): string =>
  `<svg width="${size}" height="${size}" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="square" stroke-linejoin="miter" aria-hidden="true" focusable="false">${body}</svg>`;

export const icon = {
  caution: (size?: number) => box(`<path d="M8 2 L14.5 13.5 H1.5 Z"/><path d="M8 6.5 v3.2"/><path d="M8 11.6 v.6"/>`, size),
  alert: (size?: number) => box(`<path d="M5.2 1.5 h5.6 L14.5 5.2 v5.6 L10.8 14.5 H5.2 L1.5 10.8 V5.2 Z"/><path d="M8 4.6 v4"/><path d="M8 10.6 v.6"/>`, size),
  live: (size?: number) => box(`<path d="M2 8.4 L6.2 12.5 L14 3.8"/>`, size),
  cold: (size?: number) => box(`<circle cx="8" cy="8" r="6"/><path d="M4.8 8 h6.4"/>`, size),
  /* Kit mark: a wing bar over a bounded field — the panel's own badge. */
  mark: () =>
    `<svg width="20" height="20" viewBox="0 0 20 20" fill="none" aria-hidden="true" focusable="false">
      <path d="M2 7.4 H18" stroke="var(--green)" stroke-width="1.8" stroke-linecap="square"/>
      <path d="M4.6 11.2 H15.4" stroke="var(--lum)" stroke-width="1.6" stroke-linecap="square" opacity=".75"/>
      <path d="M7.4 14.8 H12.6" stroke="var(--lum)" stroke-width="1.4" stroke-linecap="square" opacity=".45"/>
    </svg>`,
};

export function lampIcon(state: "caution" | "alert" | "live" | "cold"): string {
  return state === "alert" ? icon.alert(14) : state === "live" ? icon.live(14) : state === "caution" ? icon.caution(14) : icon.cold(14);
}
