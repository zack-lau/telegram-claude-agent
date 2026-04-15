// components/ble_audio/ble_audio.c
#include "ble_audio.h"
#include "gatt_defs.h"
#include "nimble/nimble_port.h"
#include "nimble/nimble_port_freertos.h"
#include "host/ble_hs.h"
#include "host/ble_gap.h"
#include "host/ble_gatt.h"
#include "services/gap/ble_svc_gap.h"
#include "services/gatt/ble_svc_gatt.h"
#include "esp_log.h"
#include <string.h>

static const char *TAG = "ble_audio";
static _Atomic uint16_t s_conn_handle = BLE_HS_CONN_HANDLE_NONE;
static uint16_t s_wake_val_handle = 0;
static uint16_t s_audio_val_handle = 0;
static ble_on_ctrl_cb_t s_ctrl_cb = NULL;
static ble_on_connect_cb_t s_conn_cb = NULL;
static ble_on_model_cb_t s_model_cb = NULL;
static uint32_t s_model_offset = 0;

static int gatt_ctrl_write_cb(uint16_t conn_handle, uint16_t attr_handle,
                               struct ble_gatt_access_ctxt *ctxt, void *arg) {
    if (ctxt->op != BLE_GATT_ACCESS_OP_WRITE_CHR) return 0;
    if (ctxt->om->om_len < 1) return 0;
    uint8_t cmd = ctxt->om->om_data[0];
    ESP_LOGI(TAG, "CTRL command: 0x%02x", cmd);
    if (s_ctrl_cb) s_ctrl_cb(cmd);
    return 0;
}

static int gatt_model_write_cb(uint16_t conn_handle, uint16_t attr_handle,
                                struct ble_gatt_access_ctxt *ctxt, void *arg) {
    if (ctxt->op != BLE_GATT_ACCESS_OP_WRITE_CHR) return 0;
    uint16_t len = OS_MBUF_PKTLEN(ctxt->om);
    if (len == 0) return 0;

    // Check for reset marker: 4-byte value 0x00000000 resets offset
    if (len == 4) {
        uint32_t val = 0;
        os_mbuf_copydata(ctxt->om, 0, 4, &val);
        if (val == 0) {
            s_model_offset = 0;
            ESP_LOGI(TAG, "Model upload reset");
            return 0;
        }
    }

    uint8_t *buf = malloc(len);
    if (!buf) return BLE_ATT_ERR_INSUFFICIENT_RES;
    os_mbuf_copydata(ctxt->om, 0, len, buf);
    if (s_model_cb) s_model_cb(s_model_offset, buf, len);
    s_model_offset += len;
    free(buf);
    return 0;
}

static int gatt_info_read_cb(uint16_t conn_handle, uint16_t attr_handle,
                              struct ble_gatt_access_ctxt *ctxt, void *arg) {
    // Return: battery% (dummy 100), firmware version
    uint8_t info[] = { 100, 0, 1, 0 };  // battery=100%, fw=0.1.0
    os_mbuf_append(ctxt->om, info, sizeof(info));
    return 0;
}

static const struct ble_gatt_chr_def s_chrs[] = {
    {   // Wake notify
        .uuid = BLE_UUID128_DECLARE(BLE_CHR_WAKE_UUID),
        .flags = BLE_GATT_CHR_F_NOTIFY,
        .val_handle = &s_wake_val_handle,
        .access_cb = NULL,
    },
    {   // Audio stream notify
        .uuid = BLE_UUID128_DECLARE(BLE_CHR_AUDIO_UUID),
        .flags = BLE_GATT_CHR_F_NOTIFY,
        .val_handle = &s_audio_val_handle,
        .access_cb = NULL,
    },
    {   // Control write
        .uuid = BLE_UUID128_DECLARE(BLE_CHR_CTRL_UUID),
        .flags = BLE_GATT_CHR_F_WRITE | BLE_GATT_CHR_F_WRITE_NO_RSP,
        .access_cb = gatt_ctrl_write_cb,
    },
    {   // Model upload write
        .uuid      = BLE_UUID128_DECLARE(BLE_CHR_MODEL_UUID),
        .flags     = BLE_GATT_CHR_F_WRITE | BLE_GATT_CHR_F_WRITE_NO_RSP,
        .access_cb = gatt_model_write_cb,
    },
    {   // Device info read
        .uuid = BLE_UUID128_DECLARE(BLE_CHR_INFO_UUID),
        .flags = BLE_GATT_CHR_F_READ,
        .access_cb = gatt_info_read_cb,
    },
    { 0 },  // terminator
};

static const struct ble_gatt_svc_def s_svcs[] = {
    {
        .type = BLE_GATT_SVC_TYPE_PRIMARY,
        .uuid = BLE_UUID128_DECLARE(BLE_SVC_UUID),
        .characteristics = s_chrs,
    },
    { 0 },  // terminator
};

static int gap_event_handler(struct ble_gap_event *event, void *arg) {
    switch (event->type) {
        case BLE_GAP_EVENT_CONNECT:
            if (event->connect.status == 0) {
                s_conn_handle = event->connect.conn_handle;
                ESP_LOGI(TAG, "BLE connected, handle=%d", s_conn_handle);
                if (s_conn_cb) s_conn_cb(true);
            }
            break;
        case BLE_GAP_EVENT_DISCONNECT:
            ESP_LOGI(TAG, "BLE disconnected, reason=%d", event->disconnect.reason);
            s_conn_handle = BLE_HS_CONN_HANDLE_NONE;
            if (s_conn_cb) s_conn_cb(false);
            ble_audio_start_advertising();  // re-advertise
            break;
        case BLE_GAP_EVENT_MTU:
            ESP_LOGI(TAG, "MTU updated: %d", event->mtu.value);
            break;
        default: break;
    }
    return 0;
}

static void ble_on_sync(void) {
    ble_audio_start_advertising();
}

static void nimble_host_task(void *arg) {
    nimble_port_run();
    nimble_port_freertos_deinit();
}

esp_err_t ble_audio_init(ble_on_ctrl_cb_t ctrl_cb, ble_on_connect_cb_t conn_cb,
                         ble_on_model_cb_t model_cb) {
    s_ctrl_cb  = ctrl_cb;
    s_conn_cb  = conn_cb;
    s_model_cb = model_cb;

    // ESP-IDF 5.x: nimble_port_init() handles HCI + controller init internally.
    // (In IDF 4.x this required a separate esp_nimble_hci_and_controller_init() call.)
    nimble_port_init();
    ble_svc_gap_init();
    ble_svc_gatt_init();

    int rc = ble_gatts_count_cfg(s_svcs);
    if (rc != 0) { ESP_LOGE(TAG, "ble_gatts_count_cfg: %d", rc); return ESP_FAIL; }
    rc = ble_gatts_add_svcs(s_svcs);
    if (rc != 0) { ESP_LOGE(TAG, "ble_gatts_add_svcs: %d", rc); return ESP_FAIL; }

    ble_hs_cfg.sync_cb = ble_on_sync;
    ble_hs_cfg.store_status_cb = ble_store_util_status_rr;

    // LE Secure Connections bonding (spec requirement)
    ble_hs_cfg.sm_sc      = 1;   // Secure Connections
    ble_hs_cfg.sm_bonding = 1;   // Bond after pairing
    ble_hs_cfg.sm_mitm    = 1;   // Require MITM protection
    ble_hs_cfg.sm_io_cap  = BLE_SM_IO_CAP_NO_IO; // Just Works (no display/input on device)

    ble_svc_gap_device_name_set("esp32-mic");

    nimble_port_freertos_init(nimble_host_task);
    ESP_LOGI(TAG, "BLE initialized");
    return ESP_OK;
}

esp_err_t ble_audio_start_advertising(void) {
    struct ble_gap_adv_params adv_params = {
        .conn_mode = BLE_GAP_CONN_MODE_UND,
        .disc_mode = BLE_GAP_DISC_MODE_GEN,
        .itvl_min  = BLE_GAP_ADV_ITVL_MS(100),
        .itvl_max  = BLE_GAP_ADV_ITVL_MS(200),
    };
    static const char s_adv_name[] = "esp32-mic";
    struct ble_hs_adv_fields fields = {
        .flags = BLE_HS_ADV_F_DISC_GEN | BLE_HS_ADV_F_BREDR_UNSUP,
        .name = (const uint8_t *)s_adv_name,
        .name_len = sizeof(s_adv_name) - 1,
        .name_is_complete = 1,
    };
    ble_gap_adv_set_fields(&fields);
    int rc = ble_gap_adv_start(BLE_OWN_ADDR_PUBLIC, NULL, BLE_HS_FOREVER,
                                &adv_params, gap_event_handler, NULL);
    if (rc != 0 && rc != BLE_HS_EALREADY) {
        ESP_LOGE(TAG, "ble_gap_adv_start: %d", rc);
        return ESP_FAIL;
    }
    ESP_LOGI(TAG, "BLE advertising as 'esp32-mic'");
    return ESP_OK;
}

esp_err_t ble_audio_send_wake(void) {
    if (s_conn_handle == BLE_HS_CONN_HANDLE_NONE) return ESP_ERR_NOT_FOUND;
    uint8_t val = 0x01;
    struct os_mbuf *om = ble_hs_mbuf_from_flat(&val, 1);
    if (!om) return ESP_ERR_NO_MEM;
    int rc = ble_gatts_notify_custom(s_conn_handle, s_wake_val_handle, om);
    if (rc != 0) {
        os_mbuf_free_chain(om);
        return ESP_FAIL;
    }
    return ESP_OK;
}

esp_err_t ble_audio_send_frame(const uint8_t *opus_data, size_t len) {
    if (s_conn_handle == BLE_HS_CONN_HANDLE_NONE) return ESP_ERR_NOT_FOUND;
    struct os_mbuf *om = ble_hs_mbuf_from_flat(opus_data, len);
    if (!om) return ESP_ERR_NO_MEM;
    int rc = ble_gatts_notify_custom(s_conn_handle, s_audio_val_handle, om);
    if (rc != 0) {
        os_mbuf_free_chain(om);
        return ESP_FAIL;
    }
    return ESP_OK;
}

bool ble_audio_is_connected(void) {
    return s_conn_handle != BLE_HS_CONN_HANDLE_NONE;
}
