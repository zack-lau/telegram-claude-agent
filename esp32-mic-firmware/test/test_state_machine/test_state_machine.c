#include <assert.h>
#include <stdio.h>
#include "state_machine.h"

static device_state_t last_state = -1;
static void on_transition(device_state_t new_state) { last_state = new_state; }

static void test_initial_state(void) {
    last_state = -1;
    sm_t sm;
    sm_init(&sm, on_transition);
    assert(sm_get_state(&sm) == STATE_STANDBY);
    printf("PASS: initial state is STANDBY\n");
}

static void test_wake_triggers_streaming(void) {
    last_state = -1;
    sm_t sm;
    sm_init(&sm, on_transition);
    sm_handle_event(&sm, EVENT_WAKE_DETECTED);
    assert(sm_get_state(&sm) == STATE_STREAMING);
    assert(last_state == STATE_STREAMING);
    printf("PASS: wake word -> STREAMING\n");
}

static void test_button_short_triggers_streaming(void) {
    last_state = -1;
    sm_t sm;
    sm_init(&sm, on_transition);
    sm_handle_event(&sm, EVENT_BUTTON_SHORT);
    assert(sm_get_state(&sm) == STATE_STREAMING);
    printf("PASS: button short press -> STREAMING\n");
}

static void test_silence_timeout_triggers_idle_confirm(void) {
    last_state = -1;
    sm_t sm;
    sm_init(&sm, on_transition);
    sm_handle_event(&sm, EVENT_WAKE_DETECTED);
    sm_handle_event(&sm, EVENT_SILENCE_TIMEOUT);
    assert(sm_get_state(&sm) == STATE_IDLE_CONFIRM);
    printf("PASS: silence timeout -> IDLE_CONFIRM\n");
}

static void test_button_long_cancels_to_standby(void) {
    last_state = -1;
    sm_t sm;
    sm_init(&sm, on_transition);
    sm_handle_event(&sm, EVENT_WAKE_DETECTED);
    sm_handle_event(&sm, EVENT_BUTTON_LONG);
    assert(sm_get_state(&sm) == STATE_STANDBY);
    printf("PASS: button long press -> STANDBY (cancel)\n");
}

static void test_confirm_done_returns_to_standby(void) {
    last_state = -1;
    sm_t sm;
    sm_init(&sm, on_transition);
    sm_handle_event(&sm, EVENT_WAKE_DETECTED);
    sm_handle_event(&sm, EVENT_SILENCE_TIMEOUT);
    sm_handle_event(&sm, EVENT_CONFIRM_DONE);
    assert(sm_get_state(&sm) == STATE_STANDBY);
    printf("PASS: confirm done -> STANDBY\n");
}

static void test_no_wake_in_streaming(void) {
    last_state = -1;
    sm_t sm;
    sm_init(&sm, on_transition);
    sm_handle_event(&sm, EVENT_WAKE_DETECTED);
    sm_handle_event(&sm, EVENT_WAKE_DETECTED);
    assert(sm_get_state(&sm) == STATE_STREAMING);
    printf("PASS: wake ignored when already STREAMING\n");
}

int main(void) {
    test_initial_state();
    test_wake_triggers_streaming();
    test_button_short_triggers_streaming();
    test_silence_timeout_triggers_idle_confirm();
    test_button_long_cancels_to_standby();
    test_confirm_done_returns_to_standby();
    test_no_wake_in_streaming();
    printf("All tests passed.\n");
    return 0;
}
