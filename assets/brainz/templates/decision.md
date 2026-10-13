---
id: decision
name: Decision record
description: Lightweight ADR. What we decided, why, what we rejected, and when to revisit.
kind: note
icon: scale
group: Planning
modes: [new, insert, section]
path: '{{folder}}/decisions/{{date}}-{{slug input.title}}.md'
applies_to: ["**/decisions/**"]
inputs:
  - { id: title, type: text, label: Decision in a few words, required: true }
---
# Decision: {{input.title}} .. {{date}}

**Status:** decided {{date}} · **Owner:** {{owner}} · **Revisit:** {{date "+90d"}}

## Context
{{selection}}{{cursor}}

## Options considered
1. **Option A** .. pros / cons
2. **Option B** .. pros / cons

## Decision
We'll ...

## Why
-

## Consequences and risks
-

## Links
-
