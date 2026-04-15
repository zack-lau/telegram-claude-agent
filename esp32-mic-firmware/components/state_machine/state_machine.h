#pragma once

typedef enum {
    STATE_STANDBY = 0,
    STATE_STREAMING,
    STATE_IDLE_CONFIRM,
    STATE_COUNT,
} device_state_t;

typedef enum {
    EVENT_WAKE_DETECTED = 0,
    EVENT_BUTTON_SHORT,
    EVENT_BUTTON_LONG,
    EVENT_SILENCE_TIMEOUT,
    EVENT_CONFIRM_DONE,
    EVENT_COUNT,
} device_event_t;

typedef void (*sm_on_transition_cb_t)(device_state_t new_state);

typedef struct {
    device_state_t state;
    sm_on_transition_cb_t on_transition;
} sm_t;

void            sm_init(sm_t *sm, sm_on_transition_cb_t cb);
device_state_t  sm_get_state(const sm_t *sm);
void            sm_handle_event(sm_t *sm, device_event_t event);
