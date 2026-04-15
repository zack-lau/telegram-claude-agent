#pragma once
#include "esp_err.h"
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

// Called when WakeNet detects the wake word
typedef void (*afe_on_wake_cb_t)(void);

// Called with PCM audio data (16kHz, 16-bit, mono) during streaming
// buf: audio samples, len: number of int16_t samples
typedef void (*afe_on_audio_cb_t)(const int16_t *buf, size_t len);

// model_data: pointer to WakeNet model binary in flash (mmap'd from wake_model partition).
//             Pass NULL to use the default bundled model.
esp_err_t afe_pipeline_init(afe_on_wake_cb_t wake_cb, afe_on_audio_cb_t audio_cb,
                             const void *model_data);
esp_err_t afe_pipeline_start(void);
esp_err_t afe_pipeline_stop(void);
void      afe_pipeline_set_streaming(bool streaming);
