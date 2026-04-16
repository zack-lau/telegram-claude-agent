import { type Context } from "grammy";
import { getConfig, log } from "../config.js";

/**
 * Download a voice/audio file from Telegram and transcribe it via Whisper.
 * Returns the transcribed text, or null on failure.
 */
export async function transcribeVoice(ctx: Context): Promise<string | null> {
  const cfg = getConfig();

  // Get file info — voice messages use ctx.message.voice, audio uses ctx.message.audio
  if (!cfg.SPARK_WHISPER_URL) {
    log("debug", "Voice transcription skipped — SPARK_WHISPER_URL not configured");
    return null;
  }

  const voice = ctx.message?.voice;
  const audio = ctx.message?.audio;
  const fileId = voice?.file_id ?? audio?.file_id;

  if (!fileId) return null;

  try {
    // Get file path from Telegram
    const file = await ctx.api.getFile(fileId);
    if (!file.file_path) {
      log("warn", "Telegram returned no file_path for voice message");
      return null;
    }

    // Download the file bytes
    const fileUrl = `https://api.telegram.org/file/bot${cfg.TELEGRAM_BOT_TOKEN}/${file.file_path}`;
    const fileResp = await fetch(fileUrl);
    if (!fileResp.ok) {
      log("warn", `Failed to download voice file: ${fileResp.status}`);
      return null;
    }
    const fileBuffer = await fileResp.arrayBuffer();

    // Determine filename with extension for Whisper
    const ext = file.file_path.split(".").pop() ?? "ogg";
    const filename = `voice.${ext}`;

    // Send to Whisper server (OpenAI-compatible endpoint)
    const form = new FormData();
    form.append("file", new Blob([fileBuffer]), filename);
    form.append("model", "whisper-large-v3-turbo");

    const whisperResp = await fetch(
      `${cfg.SPARK_WHISPER_URL}/v1/audio/transcriptions`,
      { method: "POST", body: form },
    );

    if (!whisperResp.ok) {
      log("warn", `Whisper transcription failed: ${whisperResp.status}`);
      return null;
    }

    const result = (await whisperResp.json()) as { text?: string };
    const text = result.text?.trim();

    if (!text) {
      log("warn", "Whisper returned empty transcription");
      return null;
    }

    log("info", `Transcribed voice message: ${text.length} chars`);
    return text;
  } catch (err) {
    log("error", "Voice transcription error", err);
    return null;
  }
}
