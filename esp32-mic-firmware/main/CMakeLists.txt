idf_component_register(
    SRCS "main.c"
    INCLUDE_DIRS "."
    REQUIRES
        state_machine
        led_driver
        button_driver
        afe_pipeline
        opus_encoder
        ble_audio
        wake_model
        nvs_flash
        freertos
        esp_log
        esp_timer
)
