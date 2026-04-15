#include "state_machine.h"
#include <stddef.h>

// [state][event] -> next_state, or -1 if ignored
static const int TRANSITIONS[STATE_COUNT][EVENT_COUNT] = {
    //                   WAKE              BTN_SHORT         BTN_LONG       SILENCE             CONFIRM
    [STATE_STANDBY]     = { STATE_STREAMING,  STATE_STREAMING,  -1,            -1,                 -1            },
    [STATE_STREAMING]   = { -1,               -1,               STATE_STANDBY, STATE_IDLE_CONFIRM, -1            },
    [STATE_IDLE_CONFIRM]= { -1,               -1,               -1,            -1,                 STATE_STANDBY },
};

void sm_init(sm_t *sm, sm_on_transition_cb_t cb) {
    sm->state = STATE_STANDBY;
    sm->on_transition = cb;
}

device_state_t sm_get_state(const sm_t *sm) {
    return sm->state;
}

void sm_handle_event(sm_t *sm, device_event_t event) {
    if (sm == NULL || sm->state >= STATE_COUNT || event >= EVENT_COUNT) return;
    int next = TRANSITIONS[sm->state][event];
    if (next == -1) return;
    sm->state = (device_state_t)next;
    if (sm->on_transition) sm->on_transition(sm->state);
}
