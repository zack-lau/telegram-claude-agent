#include "afe_pipeline.h"
#include "driver/i2s_std.h"
#include "esp_afe_sr_models.h"
#include "esp_afe_sr_iface.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "freertos/semphr.h"
#include "esp_log.h"
#include "esp_heap_caps.h"
#include <assert.h>
#include <string.h>

#define I2S_SCK_GPIO    14
#define I2S_WS_GPIO     15
#define I2S_SD_GPIO     16
#define SAMPLE_RATE     16000
#define DMA_BUF_COUNT   4
#define DMA_BUF_LEN     512

static const char *TAG = "afe";
static i2s_chan_handle_t s_rx_chan = NULL;
static const esp_afe_sr_iface_t *s_afe_handle = NULL;
static esp_afe_sr_data_t *s_afe_data = NULL;
static afe_on_wake_cb_t s_wake_cb = NULL;
static afe_on_audio_cb_t s_audio_cb = NULL;
static volatile bool s_streaming = false;
static volatile bool s_afe_running = false;
static SemaphoreHandle_t s_stop_sem = NULL;
static TaskHandle_t s_afe_task = NULL;

static esp_err_t i2s_init(void) {
    i2s_chan_config_t chan_cfg = I2S_CHANNEL_DEFAULT_CONFIG(I2S_NUM_0, I2S_ROLE_MASTER);
    chan_cfg.dma_desc_num = DMA_BUF_COUNT;
    chan_cfg.dma_frame_num = DMA_BUF_LEN;
    ESP_ERROR_CHECK(i2s_new_channel(&chan_cfg, NULL, &s_rx_chan));

    i2s_std_config_t std_cfg = {
        .clk_cfg = I2S_STD_CLK_DEFAULT_CONFIG(SAMPLE_RATE),
        .slot_cfg = I2S_STD_PHILIPS_SLOT_DEFAULT_CONFIG(I2S_DATA_BIT_WIDTH_32BIT, I2S_SLOT_MODE_MONO),
        .gpio_cfg = {
            .bclk = I2S_SCK_GPIO,
            .ws   = I2S_WS_GPIO,
            .dout = I2S_GPIO_UNUSED,
            .din  = I2S_SD_GPIO,
            .invert_flags = { .mclk_inv = false, .bclk_inv = false, .ws_inv = false },
        },
    };
    ESP_ERROR_CHECK(i2s_channel_init_std_mode(s_rx_chan, &std_cfg));
    return ESP_OK;
}

// INMP441 outputs 24-bit data in 32-bit frames (left-justified, MSB, bits 31..8 valid).
// Shift right by 16 to get signed 16-bit audio.
static void convert_32bit_to_16bit(const int32_t *in, int16_t *out, size_t num_samples) {
    for (size_t i = 0; i < num_samples; i++) {
        out[i] = (int16_t)(in[i] >> 16);
    }
}

static void afe_task(void *arg) {
    int afe_chunk = s_afe_handle->get_feed_chunksize(s_afe_data);
    size_t raw_bytes = (size_t)afe_chunk * sizeof(int32_t);
    int32_t *raw_buf = heap_caps_malloc(raw_bytes, MALLOC_CAP_DMA);
    int16_t *pcm_buf = malloc((size_t)afe_chunk * sizeof(int16_t));

    if (!raw_buf || !pcm_buf) {
        ESP_LOGE(TAG, "AFE buffer alloc failed (chunk=%d)", afe_chunk);
        heap_caps_free(raw_buf);
        free(pcm_buf);
        if (s_stop_sem) xSemaphoreGive(s_stop_sem);
        vTaskDelete(NULL);
        return;
    }

    ESP_LOGI(TAG, "AFE task started, chunk=%d samples", afe_chunk);

    s_afe_running = true;
    while (s_afe_running) {
        size_t bytes_read = 0;
        esp_err_t err = i2s_channel_read(s_rx_chan, raw_buf, raw_bytes, &bytes_read,
                                          pdMS_TO_TICKS(200));
        if (err != ESP_OK) continue;

        size_t samples_read = bytes_read / sizeof(int32_t);
        if (samples_read != (size_t)afe_chunk) continue;  // skip partial reads

        convert_32bit_to_16bit(raw_buf, pcm_buf, samples_read);
        s_afe_handle->feed(s_afe_data, pcm_buf);

        afe_fetch_result_t *res = s_afe_handle->fetch(s_afe_data);
        if (!res) continue;

        if (res->wakeup_state == WAKENET_DETECTED) {
            ESP_LOGI(TAG, "Wake word detected (wakenet_output=%d)", res->wakenet_output);
            if (s_wake_cb) s_wake_cb();
        }

        if (s_streaming && s_audio_cb && res->data && res->data_size > 0) {
            s_audio_cb(res->data, res->data_size / sizeof(int16_t));
        }
    }

    // Clean exit: free buffers and signal stop semaphore
    heap_caps_free(raw_buf);
    free(pcm_buf);
    if (s_stop_sem) xSemaphoreGive(s_stop_sem);
    vTaskDelete(NULL);
}

esp_err_t afe_pipeline_init(afe_on_wake_cb_t wake_cb, afe_on_audio_cb_t audio_cb,
                             const void *model_data) {
    s_wake_cb = wake_cb;
    s_audio_cb = audio_cb;

    s_stop_sem = xSemaphoreCreateBinary();
    if (!s_stop_sem) return ESP_ERR_NO_MEM;

    esp_err_t err = i2s_init();
    if (err != ESP_OK) {
        vSemaphoreDelete(s_stop_sem);
        s_stop_sem = NULL;
        return err;
    }

    s_afe_handle = &ESP_AFE_SR_HANDLE;
    afe_config_t afe_cfg = AFE_CONFIG_DEFAULT();
    afe_cfg.aec_init    = false;
    afe_cfg.se_init     = true;
    afe_cfg.vad_init    = true;
    afe_cfg.wakenet_init = true;
    afe_cfg.wakenet_mode = DET_MODE_90;
    afe_cfg.afe_mode    = SR_MODE_LOW_COST;

    if (model_data != NULL) {
        // Use custom WakeNet model from wake_model partition
        // esp-sr >= 2.0: set wakenet_model_select to point to custom binary
#ifdef AFE_CONFIG_HAS_WAKENET_MODEL_SELECT
        afe_cfg.wakenet_model_select = (char *)model_data;
        ESP_LOGI(TAG, "AFE using custom wake model from partition");
#else
        ESP_LOGW(TAG, "Custom wake model: wakenet_model_select not available in this esp-sr version — using default");
#endif
    }

    s_afe_data = s_afe_handle->create_from_config(&afe_cfg);
    if (!s_afe_data) {
        ESP_LOGE(TAG, "AFE create_from_config failed");
        i2s_channel_disable(s_rx_chan);
        i2s_del_channel(s_rx_chan);
        s_rx_chan = NULL;
        vSemaphoreDelete(s_stop_sem);
        s_stop_sem = NULL;
        return ESP_ERR_NO_MEM;
    }

    ESP_LOGI(TAG, "AFE pipeline initialized");
    return ESP_OK;
}

esp_err_t afe_pipeline_start(void) {
    ESP_ERROR_CHECK(i2s_channel_enable(s_rx_chan));
    if (xTaskCreatePinnedToCore(afe_task, "afe", 8192, NULL, 12, &s_afe_task, 1) != pdPASS) {
        ESP_LOGE(TAG, "Failed to create AFE task");
        i2s_channel_disable(s_rx_chan);
        return ESP_ERR_NO_MEM;
    }
    ESP_LOGI(TAG, "AFE pipeline started on Core 1");
    return ESP_OK;
}

esp_err_t afe_pipeline_stop(void) {
    if (s_afe_task) {
        s_afe_running = false;
        // Wait up to 2s for task to exit cleanly
        if (s_stop_sem) {
            xSemaphoreTake(s_stop_sem, pdMS_TO_TICKS(2000));
        }
        s_afe_task = NULL;
    }
    if (s_rx_chan) {
        i2s_channel_disable(s_rx_chan);
    }
    return ESP_OK;
}

void afe_pipeline_set_streaming(bool streaming) {
    s_streaming = streaming;
}
