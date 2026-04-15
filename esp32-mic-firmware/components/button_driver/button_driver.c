#include "button_driver.h"
#include "driver/gpio.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "freertos/queue.h"
#include "esp_log.h"
#include "esp_timer.h"

#define BUTTON_GPIO     10
#define DEBOUNCE_MS     50
#define LONG_PRESS_MS   2000

static const char *TAG = "button";
static button_cb_t s_cb = NULL;
static QueueHandle_t s_gpio_evt_queue = NULL;

static void IRAM_ATTR gpio_isr_handler(void *arg) {
    uint32_t level = gpio_get_level(BUTTON_GPIO);
    xQueueSendFromISR(s_gpio_evt_queue, &level, NULL);
}

static void button_task(void *arg) {
    int64_t press_time = 0;
    while (1) {
        uint32_t dummy;
        if (xQueueReceive(s_gpio_evt_queue, &dummy, portMAX_DELAY)) {
            vTaskDelay(pdMS_TO_TICKS(DEBOUNCE_MS));
            xQueueReset(s_gpio_evt_queue);
            uint32_t level = gpio_get_level(BUTTON_GPIO);

            if (level == 0) {  // pressed (active low)
                press_time = esp_timer_get_time();
            } else {           // released
                if (press_time == 0) continue;
                int64_t duration_ms = (esp_timer_get_time() - press_time) / 1000;
                press_time = 0;
                if (s_cb) {
                    s_cb(duration_ms >= LONG_PRESS_MS
                        ? BUTTON_EVENT_LONG_PRESS
                        : BUTTON_EVENT_SHORT_PRESS);
                }
            }
        }
    }
}

esp_err_t button_driver_init(button_cb_t cb) {
    s_cb = cb;
    s_gpio_evt_queue = xQueueCreate(4, sizeof(uint32_t));
    if (!s_gpio_evt_queue) {
        ESP_LOGE(TAG, "Failed to create button queue");
        return ESP_ERR_NO_MEM;
    }

    gpio_config_t cfg = {
        .pin_bit_mask = (1ULL << BUTTON_GPIO),
        .mode = GPIO_MODE_INPUT,
        .pull_up_en = GPIO_PULLUP_ENABLE,
        .pull_down_en = GPIO_PULLDOWN_DISABLE,
        .intr_type = GPIO_INTR_ANYEDGE,
    };
    ESP_ERROR_CHECK(gpio_config(&cfg));
    esp_err_t isr_err = gpio_install_isr_service(0);
    if (isr_err != ESP_OK && isr_err != ESP_ERR_INVALID_STATE) {
        ESP_LOGE(TAG, "gpio_install_isr_service failed: %d", isr_err);
        return isr_err;
    }
    ESP_ERROR_CHECK(gpio_isr_handler_add(BUTTON_GPIO, gpio_isr_handler, NULL));

    if (xTaskCreate(button_task, "button", 2048, NULL, 10, NULL) != pdPASS) {
        ESP_LOGE(TAG, "Failed to create button task");
        return ESP_ERR_NO_MEM;
    }

    ESP_LOGI(TAG, "Button driver initialized on GPIO%d", BUTTON_GPIO);
    return ESP_OK;
}
