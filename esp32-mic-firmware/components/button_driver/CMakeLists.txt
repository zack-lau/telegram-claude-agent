idf_component_register(
    SRCS "button_driver.c"
    INCLUDE_DIRS "."
    REQUIRES driver freertos esp_log esp_timer
)
