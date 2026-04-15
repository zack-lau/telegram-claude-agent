// main/main.c
#include <stdio.h>
#include <string.h>
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "freertos/queue.h"
#include "freertos/semphr.h"
#include "esp_log.h"
#include "esp_timer.h"
#include "nvs_flash.h"

#include "state_machine.h"
#include "led_driver.h"
#include "button_driver.h"
#include "afe_pipeline.h"
#include "opus_encoder.h"
#include "ble_audio.h"

static const char *TAG = "main";

// ── State ────────────────────────────────────────────────────────────────────
static sm_t s_sm;
static QueueHandle_t s_event_queue;
static esp_timer_handle_t s_confirm_timer = NULL;
static SemaphoreHandle_t s_audio_mutex = NULL;

static void post_event(device_event_t evt) {
    if (xQueueSend(s_event_queue, &evt, 0) != pdTRUE) {
        ESP_LOGW(TAG, "Event queue full, dropped event %d", (int)evt);
    }
}

// ── Silence detection ────────────────────────────────────────────────────────
#define SILENCE_THRESHOLD   200    // int16 amplitude
#define SILENCE_TIMEOUT_MS  1500

static int64_t s_last_voice_ms = 0;

static bool is_silent(const int16_t *buf, size_t len) {
    for (size_t i = 0; i < len; i++) {
        int32_t sample = buf[i];
        if (sample < 0) sample = -sample;
        if (sample > SILENCE_THRESHOLD) return false;
    }
    return true;
}

// ── AFE callbacks ─────────────────────────────────────────────────────────────
static uint8_t s_opus_buf[OPUS_MAX_PACKET];
static int16_t s_pcm_accum[OPUS_FRAME_SAMPLES];
static size_t  s_pcm_accum_idx = 0;

static void on_audio(const int16_t *buf, size_t len) {
    // Silence detection
    if (!is_silent(buf, len)) {
        xSemaphoreTake(s_audio_mutex, portMAX_DELAY);
        s_last_voice_ms = (int64_t)xTaskGetTickCount() * portTICK_PERIOD_MS;
        xSemaphoreGive(s_audio_mutex);
    } else {
        int64_t now = (int64_t)xTaskGetTickCount() * portTICK_PERIOD_MS;
        xSemaphoreTake(s_audio_mutex, portMAX_DELAY);
        int64_t last = s_last_voice_ms;
        xSemaphoreGive(s_audio_mutex);
        if (last > 0 && (now - last) > SILENCE_TIMEOUT_MS) {
            xSemaphoreTake(s_audio_mutex, portMAX_DELAY);
            s_last_voice_ms = 0;
            xSemaphoreGive(s_audio_mutex);
            post_event(EVENT_SILENCE_TIMEOUT);
            return;
        }
    }

    // Accumulate into OPUS frame — hold mutex to protect s_pcm_accum_idx against
    // concurrent reset from on_state_transition (main task, Core 0).
    xSemaphoreTake(s_audio_mutex, portMAX_DELAY);
    size_t remaining = len;
    const int16_t *src = buf;
    while (remaining > 0) {
        size_t copy = remaining < (OPUS_FRAME_SAMPLES - s_pcm_accum_idx)
                    ? remaining : (OPUS_FRAME_SAMPLES - s_pcm_accum_idx);
        memcpy(&s_pcm_accum[s_pcm_accum_idx], src, copy * sizeof(int16_t));
        s_pcm_accum_idx += copy;
        src += copy;
        remaining -= copy;

        if (s_pcm_accum_idx == OPUS_FRAME_SAMPLES) {
            int bytes = opus_enc_encode(s_pcm_accum, s_opus_buf);
            if (bytes > 0) {
                ble_audio_send_frame(s_opus_buf, (size_t)bytes);
            }
            s_pcm_accum_idx = 0;
        }
    }
    xSemaphoreGive(s_audio_mutex);
}

static void on_wake(void) {
    post_event(EVENT_WAKE_DETECTED);
}

// ── Button callback ───────────────────────────────────────────────────────────
static void on_button(button_event_t evt) {
    post_event(evt == BUTTON_EVENT_SHORT_PRESS
        ? EVENT_BUTTON_SHORT : EVENT_BUTTON_LONG);
}

// ── BLE callbacks ─────────────────────────────────────────────────────────────
static void on_ble_ctrl(uint8_t cmd) {
    if (cmd == BLE_CTRL_STOP)   post_event(EVENT_SILENCE_TIMEOUT);
    if (cmd == BLE_CTRL_CANCEL) post_event(EVENT_BUTTON_LONG);
}

static void on_ble_conn(bool connected) {
    ESP_LOGI(TAG, "BLE %s", connected ? "connected" : "disconnected");
}

// ── Confirm timer callback ─────────────────────────────────────────────────────
static void confirm_timer_cb(void *arg) {
    post_event(EVENT_CONFIRM_DONE);
}

// ── State transition handler ──────────────────────────────────────────────────
static void on_state_transition(device_state_t new_state) {
    ESP_LOGI(TAG, "State → %s",
        new_state == STATE_STANDBY      ? "STANDBY" :
        new_state == STATE_STREAMING    ? "STREAMING" :
        new_state == STATE_IDLE_CONFIRM ? "IDLE_CONFIRM" : "?");

    switch (new_state) {
        case STATE_STANDBY:
            afe_pipeline_set_streaming(false);
            xSemaphoreTake(s_audio_mutex, portMAX_DELAY);
            s_pcm_accum_idx = 0;
            s_last_voice_ms = 0;
            xSemaphoreGive(s_audio_mutex);
            led_driver_set_pattern(LED_PATTERN_SLOW_BREATHE);
            break;

        case STATE_STREAMING:
            ble_audio_send_wake();
            afe_pipeline_set_streaming(true);
            xSemaphoreTake(s_audio_mutex, portMAX_DELAY);
            s_last_voice_ms = (int64_t)xTaskGetTickCount() * portTICK_PERIOD_MS;
            xSemaphoreGive(s_audio_mutex);
            led_driver_set_pattern(LED_PATTERN_SOLID_ON);
            break;

        case STATE_IDLE_CONFIRM:
            afe_pipeline_set_streaming(false);
            led_driver_set_pattern(LED_PATTERN_TRIPLE_FLASH);
            esp_timer_start_once(s_confirm_timer, 3000 * 1000);  // 3s in microseconds (spec: "3s LED flash")
            break;
    }
}

// ── Main ──────────────────────────────────────────────────────────────────────
void app_main(void) {
    ESP_LOGI(TAG, "esp32-mic v0.1.0 booting");

    // NVS (required by BLE)
    esp_err_t ret = nvs_flash_init();
    if (ret == ESP_ERR_NVS_NO_FREE_PAGES || ret == ESP_ERR_NVS_NEW_VERSION_FOUND) {
        ESP_ERROR_CHECK(nvs_flash_erase());
        ret = nvs_flash_init();
    }
    ESP_ERROR_CHECK(ret);

    s_audio_mutex = xSemaphoreCreateMutex();
    // Event queue
    s_event_queue = xQueueCreate(8, sizeof(device_event_t));

    // Components
    sm_init(&s_sm, on_state_transition);
    ESP_ERROR_CHECK(led_driver_init());
    ESP_ERROR_CHECK(button_driver_init(on_button));
    ESP_ERROR_CHECK(opus_enc_init());
    ESP_ERROR_CHECK(afe_pipeline_init(on_wake, on_audio, NULL));
    ESP_ERROR_CHECK(ble_audio_init(on_ble_ctrl, on_ble_conn, NULL));
    ESP_ERROR_CHECK(afe_pipeline_start());

    led_driver_set_pattern(LED_PATTERN_SLOW_BREATHE);
    ESP_LOGI(TAG, "System ready. Listening for wake word...");

    // Confirm timer (one-shot, fires EVENT_CONFIRM_DONE after 1s)
    const esp_timer_create_args_t timer_args = {
        .callback = confirm_timer_cb,
        .name = "confirm",
    };
    ESP_ERROR_CHECK(esp_timer_create(&timer_args, &s_confirm_timer));

    // Event loop
    device_event_t evt;
    while (1) {
        if (xQueueReceive(s_event_queue, &evt, portMAX_DELAY)) {
            sm_handle_event(&s_sm, evt);
        }
    }
}
