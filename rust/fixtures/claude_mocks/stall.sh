#!/bin/bash
# Mock claude CLI that emits one line then hangs indefinitely.
# Used to test turn_timeout_ms detection: the event keeps the process alive
# while producing no further output, so the read_timeout loops and eventually
# the turn_timeout fires.
echo '{"type":"assistant","event_type":"assistant"}'
sleep 100000
