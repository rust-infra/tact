{% if role -%}
# Your role

{{role}}
{% endif -%}

{% if skills_available -%}
# Available skills

{{skills_available}}

When a user message already contains a `<skill name="…">…</skill>` block, the user slash-invoked that skill — follow those instructions directly and do not call `load_skill` for the same skill. If the block includes an `ARGUMENTS:` line (Claude Code convention when the skill has no `$ARGUMENTS` placeholder), that line is the user's slash-command arguments for this invocation; apply the skill to fulfill them.

Skill loading policy for this Responses request. Do not call `load_skill` for greetings, small talk, or ordinary questions. Load a skill only when the user explicitly slash-invokes it or explicitly asks to use that skill; otherwise, answer directly or use only the tools needed for the task. A skill description must not make its own invocation mandatory.
{% endif -%}

{% if guidelines and guidelines | length > 0 -%}
# Guidelines you need to follow

{# Guidelines provide soft rules and best practices to complete a task well -#}

{% for item in guidelines -%}
- {{item}}
{% endfor %}
{% endif -%}

{% if constraints and constraints | length > 0 -%}
# Constraints that must be adhered to

{# Constraints are hard limitations that an agent must follow -#}

{% for item in constraints -%}
- {{item}}
{% endfor %}
{% endif -%}

{% if memory_guidance -%}
# Memory guidance

{{memory_guidance}}
{% endif -%}

{% if claude_md or additional -%}
# Additional context

{% if claude_md -%}
{{claude_md}}

{% endif -%}
{% if additional -%}
{{additional}}
{% endif -%}
{% endif -%}

{% if memory or dynamic_context -%}
=== DYNAMIC_BOUNDARY ===

{# Dynamic context below — may change mid-session without breaking the
   static prefix KV-cache.  Only the suffix from this point onward is
   invalidated when these sections change. #}

{% if memory -%}
## Memory

{{memory}}

{% endif -%}
{% if dynamic_context -%}
## Dynamic context

{{dynamic_context}}

{% endif -%}
{% endif -%}
