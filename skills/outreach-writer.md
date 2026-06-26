---
name: Outreach Writer
description: Write cold emails, LinkedIn DMs, and connection notes for jobs, clients, or partnerships. Pain-first structure, specific proof, one CTA. Use when reaching out to any stranger or new contact.
---

> This skill is baked into Jarvis permanently via `OUTREACH_GUIDE` in `src/main.rs`,
> which is appended to the system prompt on every turn. The hard rule Jarvis follows:
> before writing ANY outreach, it researches the real prospect (web_search +
> extract_contacts), uses only verified facts, and never fabricates. This file is the
> human-readable source of that method.

## Overview

This skill writes high-converting outreach messages: cold emails, LinkedIn connection notes, and LinkedIn DMs. The output sounds like a real person wrote it. No flattery. No marketing language. No em dashes.

Use it whenever someone needs to reach out to a stranger to find a job, get a client, build a partnership, or start a conversation with someone they do not know yet.

## What to gather first (Jarvis does this by searching)

About the sender: what they built/shipped (real numbers), what they do in one sentence, what they want (clients, job, partnership).

About the target: name, company, role; something specific and verified about them (a post, a launch, a problem their company faces); the pain they live with daily.

## Cold email

Subject - pick ONE, do not mix:
- Name their pain: say the thing they deal with daily that no one says to their face.
- Open a fear loop: describe a dreaded scenario, leave it unresolved.
- Hold up a mirror: a sharp observation that makes them wonder how you noticed.

Body:
1. Their world: 1-3 lines of specific observation, no flattery.
2. The pain: name the exact problem in their words, why it persists, what it costs.
3. What you do: one line on what you remove from their life.
4. Proof: 2-3 specific real names or numbers relevant to them.
5. CTA: one low-friction ask. Nothing after it.

## LinkedIn connection note

300 characters max. One specific observation + one reason to connect. No pitch, no ask.

## LinkedIn DM

Send 1-2 days after they accept. Under 150 words: observation, their pain, one or two lines on what you do, 2-3 proofs, soft close.

## Job-hunting outreach

Position across four angles: technical depth, customer understanding, product thinking, business outcomes. Show what they shipped, not titles. Never say "I am looking for a job"; say what you can do for them.

## Rules for everything

- Plain English, no word chosen to impress.
- No em dashes; use commas or short sentences.
- Specific over general ("reduced churn 18% in 60 days" beats "improved retention").
- Observations over compliments.
- Exactly one ask at the end.
- Never open with "I hope this finds you well" or any filler.

## The idea

The pain is the pitch. Name the exact problem someone lives with and they feel understood. Make it so easy to say yes that saying no feels like the mistake.
