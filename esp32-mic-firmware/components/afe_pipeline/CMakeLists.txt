idf_component_register(
    SRCS "afe_pipeline.c"
    INCLUDE_DIRS "."
    REQUIRES driver esp-sr freertos esp_log esp_heap_caps
)
