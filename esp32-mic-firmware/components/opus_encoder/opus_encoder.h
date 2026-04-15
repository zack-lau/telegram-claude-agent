// components/opus_encoder/opus_encoder.h
#pragma once
#include "esp_err.h"
#include <stdint.h>
#include <stddef.h>

#define OPUS_FRAME_SAMPLES  320       // 20ms at 16kHz
#define OPUS_MAX_PACKET     256       // bytes — enough for 32kbps at 20ms

esp_err_t opus_enc_init(void);
// Returns encoded bytes written to out_buf, or -1 on error.
// in_pcm must be exactly OPUS_FRAME_SAMPLES int16_t samples (mono).
// out_buf must be at least OPUS_MAX_PACKET bytes.
int       opus_enc_encode(const int16_t *in_pcm, uint8_t *out_buf);
// Destroy the encoder and free resources.
// MUST only be called after the AFE pipeline task has stopped (i.e., after
// afe_pipeline_stop() returns), guaranteeing no concurrent opus_enc_encode calls.
void      opus_enc_destroy(void);
