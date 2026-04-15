# components/wake_model/CMakeLists.txt
idf_component_register(
    SRCS "wake_model.c"
    INCLUDE_DIRS "."
    REQUIRES esp_partition esp_log esp_system
)
