#pragma once
#include "esp_err.h"

typedef enum {
    LED_PATTERN_OFF = 0,
    LED_PATTERN_SLOW_BREATHE,   // standby — slow blue breathe
    LED_PATTERN_FAST_BLINK,     // BLE pairing — fast blue blink
    LED_PATTERN_SOLID_ON,       // streaming — solid green
    LED_PATTERN_TRIPLE_FLASH,   // idle confirm — 3x green flash then off
    LED_PATTERN_ERROR,          // error — rapid red blink
} led_pattern_t;

esp_err_t led_driver_init(void);
void      led_driver_set_pattern(led_pattern_t pattern);
