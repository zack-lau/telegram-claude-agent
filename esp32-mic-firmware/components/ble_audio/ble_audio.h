// components/ble_audio/ble_audio.h
#pragma once
#include "esp_err.h"
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

typedef void (*ble_on_ctrl_cb_t)(uint8_t cmd);
typedef void (*ble_on_connect_cb_t)(bool connected);

// Called when phone writes a model chunk. offset=byte position in stream, data/len=chunk payload.
typedef void (*ble_on_model_cb_t)(uint32_t offset, const uint8_t *data, size_t len);

esp_err_t ble_audio_init(ble_on_ctrl_cb_t ctrl_cb, ble_on_connect_cb_t conn_cb,
                         ble_on_model_cb_t model_cb);
esp_err_t ble_audio_start_advertising(void);

// Send WAKE notification to connected phone
esp_err_t ble_audio_send_wake(void);

// Send OPUS-encoded audio frame. Returns ESP_ERR_NOT_FOUND if not connected.
esp_err_t ble_audio_send_frame(const uint8_t *opus_data, size_t len);

bool      ble_audio_is_connected(void);
