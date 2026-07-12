export const CONTEXT_SERVER_HOST = "127.0.0.1";

export function isPublicHealthRequest(method: string, rawUrl: string): boolean {
  const url = new URL(rawUrl);
  return method === "GET" && url.pathname === "/healthz" && url.search === "";
}
