const MAX_BODY = 64 * 1024;

export const text = (body: string, status: number) => new Response(body, { status });

export async function readJson(req: Request): Promise<Record<string, unknown> | undefined> {
  const declared = Number(req.headers.get("Content-Length") ?? 0);
  if (declared > MAX_BODY || !req.body) return undefined;
  // The length may be missing or wrong, so the count is on what arrives.
  const reader = req.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > MAX_BODY) {
      await reader.cancel();
      return undefined;
    }
    chunks.push(value);
  }
  const raw = new TextDecoder().decode(await new Blob(chunks as BlobPart[]).arrayBuffer());
  try {
    const v = JSON.parse(raw);
    return v && typeof v === "object" && !Array.isArray(v) ? v : undefined;
  } catch {
    return undefined;
  }
}
