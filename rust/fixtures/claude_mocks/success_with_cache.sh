#!/bin/bash
# Mock claude CLI that emits a successful stream-json sequence with cache tokens.
printf '{"type":"assistant","message":{"content":"Working on it..."}}\n'
printf '{"type":"result","result":"Task completed.","usage":{"input_tokens":150,"output_tokens":50,"cache_creation_input_tokens":30,"cache_read_input_tokens":20}}\n'
exit 0
