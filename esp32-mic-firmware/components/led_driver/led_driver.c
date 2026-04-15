#include "led_driver.h"
#include "driver/ledc.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "freertos/queue.h"
#include "esp_log.h"

#define LED_R_GPIO  21
#define LED_G_GPIO  22
#define LED_B_GPIO  23
#define LEDC_FREQ   5000
#define LEDC_RES    LEDC_TIMER_8_BIT

static const char *TAG = "led";
static QueueHandle_t s_pattern_queue = NULL;
static TaskHandle_t s_led_task = NULL;

static void set_rgb(uint8_t r, uint8_t g, uint8_t b) {
    ledc_set_duty(LEDC_LOW_SPEED_MODE, LEDC_CHANNEL_0, r);
    ledc_set_duty(LEDC_LOW_SPEED_MODE, LEDC_CHANNEL_1, g);
    ledc_set_duty(LEDC_LOW_SPEED_MODE, LEDC_CHANNEL_2, b);
    ledc_update_duty(LEDC_LOW_SPEED_MODE, LEDC_CHANNEL_0);
    ledc_update_duty(LEDC_LOW_SPEED_MODE, LEDC_CHANNEL_1);
    ledc_update_duty(LEDC_LOW_SPEED_MODE, LEDC_CHANNEL_2);
}

// Check queue for a pattern change without blocking. Returns true if pattern changed.
static bool check_pattern_change(led_pattern_t *current) {
    led_pattern_t next;
    if (xQueueReceive(s_pattern_queue, &next, 0) == pdTRUE) {
        *current = next;
        return true;
    }
    return false;
}

static void led_task(void *arg) {
    led_pattern_t pattern = LED_PATTERN_OFF;
    int step = 0;
    int flash_count = 0;

    while (1) {
        // Block waiting for first pattern or a change while idle
        if (pattern == LED_PATTERN_OFF) {
            xQueueReceive(s_pattern_queue, &pattern, portMAX_DELAY);
            step = 0;
            flash_count = 0;
        }

        switch (pattern) {
            case LED_PATTERN_OFF:
                set_rgb(0, 0, 0);
                vTaskDelay(pdMS_TO_TICKS(100));
                check_pattern_change(&pattern);
                break;

            case LED_PATTERN_SLOW_BREATHE: {
                step = (step + 1) % 60;
                uint8_t v = (uint8_t)(20 + 20 * (step < 30 ? step : (60 - step)) / 30.0f);
                set_rgb(0, 0, v);
                vTaskDelay(pdMS_TO_TICKS(50));
                if (check_pattern_change(&pattern)) { step = 0; }
                break;
            }

            case LED_PATTERN_FAST_BLINK:
                set_rgb(0, 0, (step % 4 < 2) ? 80 : 0);
                step = (step + 1) % 100;
                vTaskDelay(pdMS_TO_TICKS(125));
                if (check_pattern_change(&pattern)) { step = 0; }
                break;

            case LED_PATTERN_SOLID_ON:
                set_rgb(0, 80, 0);
                vTaskDelay(pdMS_TO_TICKS(100));
                check_pattern_change(&pattern);
                break;

            case LED_PATTERN_TRIPLE_FLASH:
                if (flash_count < 6) {
                    set_rgb(0, (flash_count % 2 == 0) ? 100 : 0, 0);
                    flash_count++;
                    vTaskDelay(pdMS_TO_TICKS(150));
                } else {
                    flash_count = 0;
                    pattern = LED_PATTERN_OFF;
                    set_rgb(0, 0, 0);
                }
                if (check_pattern_change(&pattern)) { flash_count = 0; }
                break;

            case LED_PATTERN_ERROR:
                set_rgb((step % 4 < 2) ? 200 : 0, 0, 0);
                step = (step + 1) % 100;
                vTaskDelay(pdMS_TO_TICKS(80));
                if (check_pattern_change(&pattern)) { step = 0; }
                break;

            default:
                set_rgb(0, 0, 0);
                vTaskDelay(pdMS_TO_TICKS(100));
                check_pattern_change(&pattern);
                break;
        }
    }
}

esp_err_t led_driver_init(void) {
    s_pattern_queue = xQueueCreate(4, sizeof(led_pattern_t));
    if (!s_pattern_queue) {
        ESP_LOGE(TAG, "Failed to create pattern queue");
        return ESP_ERR_NO_MEM;
    }

    ledc_timer_config_t timer = {
        .speed_mode = LEDC_LOW_SPEED_MODE,
        .duty_resolution = LEDC_RES,
        .timer_num = LEDC_TIMER_0,
        .freq_hz = LEDC_FREQ,
        .clk_cfg = LEDC_AUTO_CLK,
    };
    ESP_ERROR_CHECK(ledc_timer_config(&timer));

    int gpios[] = {LED_R_GPIO, LED_G_GPIO, LED_B_GPIO};
    for (int i = 0; i < 3; i++) {
        ledc_channel_config_t ch = {
            .gpio_num = gpios[i],
            .speed_mode = LEDC_LOW_SPEED_MODE,
            .channel = (ledc_channel_t)i,
            .timer_sel = LEDC_TIMER_0,
            .duty = 0,
            .hpoint = 0,
        };
        ESP_ERROR_CHECK(ledc_channel_config(&ch));
    }

    if (xTaskCreate(led_task, "led", 2048, NULL, 5, &s_led_task) != pdPASS) {
        ESP_LOGE(TAG, "Failed to create LED task");
        return ESP_ERR_NO_MEM;
    }

    ESP_LOGI(TAG, "LED driver initialized");
    return ESP_OK;
}

void led_driver_set_pattern(led_pattern_t pattern) {
    if (s_pattern_queue) {
        // Overwrite old pending pattern if queue is full — latest pattern wins
        led_pattern_t discard;
        while (xQueueReceive(s_pattern_queue, &discard, 0) == pdTRUE) {}
        xQueueSend(s_pattern_queue, &pattern, 0);
    }
}
