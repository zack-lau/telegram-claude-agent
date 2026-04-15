# components/opus_encoder/CMakeLists.txt
idf_component_register(
    SRCS "opus_encoder.c"
    INCLUDE_DIRS "."
    REQUIRES espressif__opus esp_log
)
