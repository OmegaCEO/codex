import importlib.util
from pathlib import Path


SCRIPT = Path(__file__).with_name("codex_compaction_repair.py")
SPEC = importlib.util.spec_from_file_location("codex_compaction_repair", SCRIPT)
repair = importlib.util.module_from_spec(SPEC)
assert SPEC.loader
SPEC.loader.exec_module(repair)


def line(value):
    import json

    return json.dumps(value)


def test_slim_drops_backend_and_scrubs_images():
    content = "\n".join(
        [
            line({"type": "session_meta", "payload": {"id": "first"}}),
            line(
                {
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": "old user"}],
                    },
                }
            ),
            line({"type": "response_item", "payload": {"type": "function_call"}}),
            line({"type": "response_item", "payload": {"type": "function_call_output"}}),
            line({"type": "response_item", "payload": {"type": "reasoning"}}),
            line({"type": "compacted", "payload": {"replacement_history": ["huge"]}}),
            line(
                {
                    "type": "event_msg",
                    "payload": {
                        "type": "agent_message",
                        "message": "Error running remote compact task: /responses/compact",
                    },
                }
            ),
            line(
                {
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "new assistant"}],
                    },
                }
            ),
            line(
                {
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "user",
                        "content": [
                            {"type": "input_text", "text": "new user"},
                            {
                                "type": "input_image",
                                "image_url": "data:image/png;base64,abc",
                            },
                        ],
                    },
                }
            ),
            line({"type": "session_meta", "payload": {"id": "last"}}),
        ]
    )

    lines, report = repair.slim_rollout_content(
        content,
        original_bytes=len(content),
        keep_all_visible=False,
        keep_visible_messages=2,
    )

    repaired = "\n".join(lines)
    assert "new assistant" in repaired
    assert "new user" in repaired
    assert "old user" not in repaired
    assert "function_call" not in repaired
    assert "reasoning" not in repaired
    assert "/responses/compact" not in repaired
    assert "data:image/png" not in repaired
    assert repair.IMAGE_REPLACEMENT in repaired
    assert report["data_image_replacements"] == 1
    assert report["compact_error_like_lines_dropped"] == 1


def test_keep_all_visible_messages():
    content = "\n".join(
        [
            line({"type": "session_meta", "payload": {"id": "first"}}),
            line(
                {
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": "old user"}],
                    },
                }
            ),
            line(
                {
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "assistant"}],
                    },
                }
            ),
        ]
    )
    lines, _ = repair.slim_rollout_content(
        content,
        original_bytes=len(content),
        keep_all_visible=True,
        keep_visible_messages=1,
    )
    repaired = "\n".join(lines)
    assert "old user" in repaired
    assert "assistant" in repaired


def test_standard_profile_keeps_500_visible_messages_by_default():
    class Args:
        profile = "standard"
        keep_all_visible_messages = False
        keep_visible_messages = None
        keep_turn_context = None
        keep_event_markers = None

    profile = repair.resolve_profile(Args())
    assert profile["keep_all_visible"] is False
    assert profile["keep_visible_messages"] == 500
    assert profile["keep_turn_context"] == 20
    assert profile["keep_event_markers"] == 80


def test_default_profile_is_gentle_for_manual_wrapper():
    assert repair.DEFAULT_PROFILE == "gentle"
    assert repair.REPAIR_PROFILES[repair.DEFAULT_PROFILE]["keep_all_visible"] is True


def test_profile_overrides_are_applied():
    class Args:
        profile = "emergency"
        keep_all_visible_messages = False
        keep_visible_messages = 120
        keep_turn_context = 7
        keep_event_markers = 15

    profile = repair.resolve_profile(Args())
    assert profile["name"] == "emergency"
    assert profile["keep_all_visible"] is False
    assert profile["keep_visible_messages"] == 120
    assert profile["keep_turn_context"] == 7
    assert profile["keep_event_markers"] == 15


def test_compact_error_detection_is_specific():
    assert repair.is_compact_error_text(
        "Failed to run pre-sampling compact: stream disconnected"
    )
    assert not repair.is_compact_error_text(
        "Can you help design a better compaction recovery tool?"
    )


def test_redacts_token_shaped_log_values():
    text = repair.redact_sensitive(
        "Authorization: Bearer abcdefghijklmnopqrstuvwxyz api_key='sk-abcdefghijklmnop123456'"
    )
    assert "abcdefghijklmnopqrstuvwxyz" not in text
    assert "sk-abcdefghijklmnop123456" not in text
    assert "Bearer [redacted]" in text
    assert "sk-[redacted]" in text
