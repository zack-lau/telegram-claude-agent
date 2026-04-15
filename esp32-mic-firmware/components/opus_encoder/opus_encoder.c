// components/opus_encoder/opus_encoder.c
#include "opus_encoder.h"
#include "opus.h"
#include "esp_log.h"
#include <stdlib.h>

static const char *TAG = "opus_enc";
static OpusEncoder *s_encoder = NULL;

#define SAMPLE_RATE  16000
#define CHANNELS     1
#define BITRATE      32000  // 32kbps

esp_err_t opus_enc_init(void) {
    if (s_encoder != NULL) {
        ESP_LOGW(TAG, "opus_enc_init called while encoder already initialized — ignoring");
        return ESP_OK;
    }
    int err;
    s_encoder = opus_encoder_create(SAMPLE_RATE, CHANNELS, OPUS_APPLICATION_VOIP, &err);
    if (err != OPUS_OK || !s_encoder) {
        ESP_LOGE(TAG, "opus_encoder_create failed: %d", err);
        return ESP_FAIL;
    }
    int ctl_err;
    ctl_err = opus_encoder_ctl(s_encoder, OPUS_SET_BITRATE(BITRATE));
    if (ctl_err != OPUS_OK) ESP_LOGW(TAG, "SET_BITRATE failed: %d", ctl_err);
    ctl_err = opus_encoder_ctl(s_encoder, OPUS_SET_COMPLEXITY(3));  // low complexity for ESP32
    if (ctl_err != OPUS_OK) ESP_LOGW(TAG, "SET_COMPLEXITY failed: %d", ctl_err);
    ctl_err = opus_encoder_ctl(s_encoder, OPUS_SET_SIGNAL(OPUS_SIGNAL_VOICE));
    if (ctl_err != OPUS_OK) ESP_LOGW(TAG, "SET_SIGNAL failed: %d", ctl_err);
    ESP_LOGI(TAG, "OPUS encoder initialized: %dHz, %dkbps, 20ms frames", SAMPLE_RATE, BITRATE/1000);
    return ESP_OK;
}

int opus_enc_encode(const int16_t *in_pcm, uint8_t *out_buf) {
    if (!s_encoder) return -1;
    opus_int32 bytes = opus_encode(s_encoder, in_pcm, OPUS_FRAME_SAMPLES, out_buf, OPUS_MAX_PACKET);
    if (bytes < 0) {
        ESP_LOGE(TAG, "opus_encode error: %s", opus_strerror((int)bytes));
        return -1;
    }
    return (int)bytes;
}

void opus_enc_destroy(void) {
    if (s_encoder) { opus_encoder_destroy(s_encoder); s_encoder = NULL; }
}
