// components/wake_model/wake_model.h
#pragma once
#include "esp_err.h"
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

// Write a new WakeNet model binary to the wake_model partition.
// Replaces existing model. Call wake_model_reload() after.
esp_err_t wake_model_write(const uint8_t *data, size_t len);

// Reload WakeNet in AFE pipeline with model from wake_model partition.
// AFE pipeline must already be initialized.
// NOTE: Hot-reload requires esp-sr >= 2.3 with reset_wakenet() API.
// Until then, triggers esp_restart() to load the new model.
esp_err_t wake_model_reload(void);

// Check if a custom model exists in the partition (vs. default).
bool wake_model_has_custom(void);
