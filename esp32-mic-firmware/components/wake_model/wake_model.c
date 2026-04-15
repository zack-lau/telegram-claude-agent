// components/wake_model/wake_model.c
#include "wake_model.h"
#include "esp_partition.h"
#include "esp_log.h"
#include "esp_system.h"
#include <string.h>

static const char *TAG = "wake_model";
#define PARTITION_LABEL "wake_model"
#define MAGIC_HEADER    0xDEADBEEF  // written at offset 0 to mark valid model

static const esp_partition_t *get_partition(void) {
    const esp_partition_t *p = esp_partition_find_first(
        ESP_PARTITION_TYPE_DATA, ESP_PARTITION_SUBTYPE_DATA_SPIFFS, PARTITION_LABEL);
    if (!p) ESP_LOGE(TAG, "wake_model partition not found!");
    return p;
}

esp_err_t wake_model_write(const uint8_t *data, size_t len) {
    if (!data) return ESP_ERR_INVALID_ARG;

    const esp_partition_t *p = get_partition();
    if (!p) return ESP_ERR_NOT_FOUND;
    if (len + 8 > p->size) {
        ESP_LOGE(TAG, "Model too large: %u bytes, partition: %lu", (unsigned)len, p->size);
        return ESP_ERR_NO_MEM;
    }

    esp_err_t err = esp_partition_erase_range(p, 0, p->size);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "Partition erase failed: %s", esp_err_to_name(err));
        return err;
    }

    uint32_t header[2] = { MAGIC_HEADER, (uint32_t)len };
    err = esp_partition_write(p, 0, header, sizeof(header));
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "Header write failed: %s", esp_err_to_name(err));
        return err;
    }

    err = esp_partition_write(p, sizeof(header), data, len);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "Model data write failed: %s", esp_err_to_name(err));
        return err;
    }

    ESP_LOGI(TAG, "Wake model written: %u bytes", (unsigned)len);
    return ESP_OK;
}

bool wake_model_has_custom(void) {
    const esp_partition_t *p = get_partition();
    if (!p) return false;
    uint32_t magic = 0;
    esp_err_t err = esp_partition_read(p, 0, &magic, sizeof(magic));
    if (err != ESP_OK) {
        ESP_LOGW(TAG, "wake_model partition read failed: %s", esp_err_to_name(err));
        return false;
    }
    return magic == MAGIC_HEADER;
}

esp_err_t wake_model_reload(void) {
    // esp-sr loads the model at AFE init time from its own internal mechanism.
    // For custom models, the recommended approach is to store the model in
    // a dedicated partition and point esp-sr's afe_config to it.
    // Full custom model hot-reload requires esp-sr >= 2.3 with the
    // esp_afe_sr_iface->reset_wakenet() API (check your esp-sr version).
    // If unavailable, a soft reboot is required: esp_restart().
    ESP_LOGW(TAG, "wake_model_reload: triggering restart to load new model");
    esp_restart();  // replaced with hot-reload API when esp-sr supports it
    while (1) {}    // unreachable — esp_restart() is noreturn; guards future hot-reload path
}
