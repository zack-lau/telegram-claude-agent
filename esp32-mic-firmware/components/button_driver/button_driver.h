#pragma once
#include "esp_err.h"

typedef enum {
    BUTTON_EVENT_SHORT_PRESS,  // < 2000ms
    BUTTON_EVENT_LONG_PRESS,   // >= 2000ms
} button_event_t;

typedef void (*button_cb_t)(button_event_t event);

esp_err_t button_driver_init(button_cb_t cb);
