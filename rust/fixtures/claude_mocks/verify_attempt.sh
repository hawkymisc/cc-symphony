#!/bin/bash
# Mock claude CLI that requires "continuation attempt" in the prompt.
# This verifies that the ClaudeRunner properly embeds the attempt value.
#
# The prompt is passed via -p argument. If it doesn't contain
# "continuation attempt", this mock returns an error.

PROMPT=""
while [[ $# -gt 0 ]]; do
    case $1 in
        -p)
            PROMPT="$2"
            shift 2
            ;;
        *)
            shift
            ;;
    esac
done

# Verify prompt contains "continuation attempt"
if [[ "$PROMPT" != *"continuation attempt"* ]]; then
    printf '{"type":"error","error":{"message":"Prompt missing continuation attempt message"}}\n'
    exit 1
fi

# Success output
printf '{"type":"assistant","message":{"content":"Working on it..."}}\n'
printf '{"type":"result","result":"Task completed successfully.","usage":{"input_tokens":100,"output_tokens":30}}\n'
exit 0
