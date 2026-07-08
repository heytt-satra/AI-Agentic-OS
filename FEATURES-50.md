# JARVIS-OS — 50-Feature Build Plan

A concrete backlog of 50 features to extend JARVIS-OS beyond its current surface
(~100 tools, encrypted memory, HUD, watchers). Grouped by area, each with a
one-line spec. Marked `[done]` as they ship. Priority favors self-contained,
verifiable, no-heavy-dependency features first (the zero-install rule holds).

Status key: `[ ]` planned, `[~]` in progress, `[x]` shipped + pushed.

## A. System & device control
1. `[ ]` open_settings — open a Windows settings page (bluetooth, wifi, display, sound) via ms-settings:
2. `[ ]` empty_recycle_bin — empty the Recycle Bin (approval-gated, shows reclaimed space)
3. `[ ]` set_volume — set the system master volume to an absolute 0-100 level
4. `[ ]` disk_usage — biggest folders under a path (where is my space going)
5. `[ ]` installed_apps — list installed applications (winget/registry)
6. `[ ]` startup_apps — list programs that run at login
7. `[ ]` brightness_set — set display brightness 0-100
8. `[ ]` clear_temp — clear the temp folder, report space reclaimed (approval-gated)
9. `[ ]` uptime_report — boot time + uptime + last N boots
10. `[ ]` gpu_status — GPU name, VRAM, utilization (if available)

## B. Files & documents
11. `[ ]` diff_files — unified diff between two files (git diff --no-index)
12. `[ ]` file_hash — SHA-256 / MD5 of a file (verify a download)
13. `[ ]` file_replace — find-and-replace text in a file (safe, count changes)
14. `[ ]` organize_folder — sort a folder's files into subfolders by type/date
15. `[ ]` find_duplicates — find duplicate files by size+hash under a path
16. `[ ]` rename_bulk — batch rename files by a pattern (approval-gated)
17. `[ ]` merge_pdfs — combine several PDFs into one
18. `[ ]` csv_query — answer a question about a CSV (columns, filters, sums)
19. `[ ]` download_file — download a URL to a path (with progress/size)
20. `[ ]` shred_file — securely delete a file (overwrite then remove)

## C. Productivity & personal data
21. `[ ]` contacts — encrypted personal address book (add/get/list/search)
22. `[ ]` todo_add/todo_list/todo_done — a user-facing to-do list (distinct from agent tasks)
23. `[ ]` timer — a countdown timer that fires a notification (backed by reminders)
24. `[ ]` world_clock — current time in another city/timezone
25. `[x]` calculator — deterministic arithmetic/expression evaluation
26. `[x]` unit_convert — convert units (length, weight, temperature, data, etc.)
27. `[ ]` currency_convert — convert currencies via a free rates API
28. `[ ]` daily_briefing — compose a morning summary (reminders, weather, watches, activity)
29. `[ ]` habit_track — track a recurring habit and show streaks
30. `[ ]` expense_log — quick expense capture + monthly total

## D. Web, research & knowledge
31. `[ ]` summarize_url — fetch a page and summarize it
32. `[ ]` translate — translate text between languages
33. `[x]` define_word — dictionary definition of a word
34. `[x]` wikipedia — summary of a topic from Wikipedia
35. `[ ]` stock_quote — current stock price + day change
36. `[ ]` crypto_price — current crypto price
37. `[ ]` rss_check — check an RSS/Atom feed for new items
38. `[ ]` hn_top — top Hacker News stories right now
39. `[ ]` github_repo — summary/latest release of a GitHub repo
40. `[ ]` maps_search — look up a place / directions (link out)

## E. Perception & media
41. `[ ]` qr_generate — make a QR code image for text/URL
42. `[ ]` qr_read — read a QR code from an image
43. `[ ]` tts_to_file — save spoken text as an audio file
44. `[ ]` image_convert — convert/resize an image (png/jpg/webp)
45. `[ ]` extract_frames — pull key frames from a video
46. `[ ]` color_pick — read the pixel color at screen coords / from an image

## F. Automation & proactivity
47. `[ ]` on_new_email_rule / watch_rss — proactive feed watcher (like page/folder watch)
48. `[ ]` scheduled_briefing — auto daily briefing at a set time
49. `[ ]` clipboard_rule — auto-act when the clipboard matches a pattern
50. `[ ]` quiet_hours — suppress nudges/notifications during set hours

---

Build order bias: A/B/C/D features needing no new native deps go first
(open_settings, diff_files, file_hash, download_file, calculator, unit_convert,
world_clock, contacts, todo, timer, daily_briefing, summarize_url, define,
wikipedia, stock/crypto, hn_top, github_repo). Media/QR/image features may add a
small pure-Rust crate. Every shipped feature is verified end-to-end and pushed.
