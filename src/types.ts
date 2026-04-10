// ── Session mapping ──

export interface SessionEntry {
  chat_id: number;
  session_id: string;
  last_active: number;
  message_count: number;
}
