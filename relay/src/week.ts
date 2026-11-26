// Monday 00:00 UTC of the week containing t, as YYYY-MM-DD.
export function weekOf(t: number): string {
  const d = new Date(t);
  const back = (d.getUTCDay() + 6) % 7;
  d.setUTCDate(d.getUTCDate() - back);
  return d.toISOString().slice(0, 10);
}
