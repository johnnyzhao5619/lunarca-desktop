# Backend localizable strings (Fluent, en-US). TDD-11 i18n.
# Only backend-generated notification/error keys live here; frontend UI copy uses react-i18next.
# Message ids are normalized from i18n keys by replacing `:`/`.` with `-`.
# Privacy: never include transcript text, clipboard content, note body, or file names.

notifications-config_recovered = Configuration was restored to safe defaults
notifications-permission_revoked = A system permission was revoked
notifications-strict_local_blocked = Strict local mode blocked a network task
notifications-prompt_catalog_rejected = The server prompt catalog is incomplete; bundled prompts are in use
notifications-prompt_catalog_degraded = The server prompt catalog matches an older manifest; some templates use bundled defaults
notifications-fallback = A status update needs attention

errors-model_missing = Local transcription model not found. Download or choose a whisper model in Settings

# System tray menu (native menu built in the backend, hence backend i18n).
tray-show_main = Open Lunarca main window
tray-text_quick_note = Text quick note
tray-voice_input = Voice input
tray-popup_translate = Popup translate
tray-screenshot_translate = Screenshot translate
tray-quick_chat = AI Chat
tray-recording = Start recording
tray-live_caption = Live captions
tray-quit = Quit Lunarca

# Default speaker track labels (export prefix / meeting minutes; mirrors common:track.*).
track-self = Me
track-remote = Them
track-remote-n = Them { $n }
track-speaker-n = Speaker { $n }

# Merging several drafts into one note (PRD-06-047 / TDD-07 §2.1). Wording lives here,
# never spelled out per-language inside the IPC layer; the stable machine prefix is
# prepended by the code.
merge-stem = Merged notes
# 标签改名（ADR-135）：名字不合法时的说明。
tag-rename-invalid = That tag name can't be used: it must not be empty, contain spaces, or start or end with /.
# 问答摘录里出处已失效的主题页声明（K4）：放在声明正文最前面。
claim-source-changed = [Source changed since cited, unverified]{" "}
claim-source-missing = [Source no longer exists, unverified]{" "}
claim-conflict = [This page records a conflicting statement; neither side is settled]{" "}

# HUD window titles (visible in the system window list / screen readers; frameless windows draw none).
window-title-popup-translate = Lunarca Popup Translate
window-title-voice-input = Lunarca Voice Input
window-title-quick-chat = Lunarca Quick Chat
window-title-text-quick-note = Lunarca Quick Note
window-title-desktop-note = Lunarca Sticky Note
window-title-recording = Lunarca Recording
window-title-teleprompter = Lunarca Teleprompter
teleprompter-no-script = No script is open in the teleprompter yet.
teleprompter-no-line = Couldn't find that line on the teleprompter. The script may have just changed.
teleprompter-retake-once = Retake · { $times }
teleprompter-retake-many = Retake ×{ $count } · { $times }
teleprompter-follow-off = Follow-along is off. Turn it on from the Teleprompter home or the panel menu.
teleprompter-clipboard-empty = There's no text on the clipboard.
teleprompter-draft-title = Script { $time }
teleprompter-file-unsupported = The teleprompter reads .txt, .md, .docx, .pdf, .html, .rtf, .doc and .odt files. Spreadsheets and scanned PDFs can't be read aloud.
window-title-screenshot = Lunarca Screenshot Translate
window-title-desktop-lens = Lunarca Translate Space
window-title-desktop-lens-trigger = Lunarca Translate Space Controls
merge-tag-stem-suffix = digest
merge-sources-heading = Sources
merge-partial-coverage = Note: there were many notes; this digest covers the first part only.

# Mark written into the body of a distilled note when generation hit an output cap (D9' 1).
# It MUST land on disk: the note is ordinary Markdown in the vault, and a DOM-only mark
# is gone the moment the note is reopened, exported, or synced to another device.
# The body does not fork by attribution (to a reader of THIS note all three caps mean the
# same thing), but it must not prescribe an action either: "regenerate" is a deterministic
# dead loop for the `caller` cap. What to do next belongs to the badge, which knows which.
distill-output-truncated-note = Note: generation hit the per-request output cap partway through, so this note is incomplete. The knowledge-note badge on the source says what to do next.

# Pre-flight on the starting line: the server has less output quota left than this note needs.
# The three numbers are passed in as arguments and are never formatted into the string in
# Rust — doing that once nails the sentence to one language, and the spot you forget to
# update hands an English-speaking user a paragraph of Chinese.
distill-quota-short = There is not enough server quota left to finish this note. Distilling it needs about { $need } output tokens, about { $remaining } remain, and it is short by about { $short }. Top up your quota, or switch to an on-device model under Compute and Models.
merge-empty = There is nothing to merge.
merge-empty-selection = Pick what you want to merge first.
merge-selection-too-small = Pick at least { $min } notes to merge; for a single note use “Write a knowledge note”.
merge-selection-too-large = At most { $max } notes per merge — please split it up.
merge-selection-invalid = One of the selected notes can’t be merged. Please pick again.
merge-aggregate-page = One of the selected items is a topic page — it is already a digest and can’t be folded in as a fragment.
merge-source-empty = One of the selected notes has no content. Please pick again.
merge-input-too-large = The selection is larger than one merge can process — please split it up.
graph-tags-knowledge-map = The knowledge map is a navigation page — it never carries relation tags.
review-not-pending = This page has no pending manual changes.
review-no-baseline = No system baseline is available to restore — use "Keep my changes" instead; the current content will be saved as your note.
review-fork-suffix = my note
ingest-needs-review = This page has pending manual changes — resolve them in the knowledge base first.
merge-source-changed = A source changed while merging, so the result was discarded. Pick again and retry.
merge-target-taken = Another write took the destination while merging, so the result was discarded. Your selection is fine — just retry.
merge-vault-switched = The vault was switched while generating, so the result was discarded.

# The two small knowledge-maintenance calls (checkup verdict / ingest point and anchor)
# when the output cap is reached before any body is produced.
# Never say "raise the output budget": those budgets are compile-time constants in
# checkup.rs / ingest.rs, and there is no such knob in the UI. Sending someone to look
# for a setting that does not exist is worse than just saying it failed.
checkup-output-capped = The model used up this call's output allowance without reaching a verdict (a reasoning model's thinking counts toward it). This claim was skipped; a less talkative model usually gets through.
# ── IPC error descriptions (backend-side; `ipc::coded_error` prepends the code) ─
# Voice: state the fact, then the next action. No second-person colloquialisms, no
# exclamation marks, no internal implementation detail.
# Privacy boundary as in the file header: paths, note bodies and transcript text never
# appear here (hence no arguments in this section).

# ── Quick AI chat (PRD-03) ─────────────────────────────────────────────────
chat-busy = The previous answer is still being generated. Please wait.
chat-empty-session = This chat is empty; there is nothing to save.
chat-message-not-found = That message is no longer part of this chat.
chat-empty-result = That message has no content that can be saved.
chat-not-chat-note = This note contains no chat records that can be loaded.
chat-writeback-no-evidence = This answer has no verifiable source in your library, so it cannot be saved to the knowledge base.
chat-writeback-unverifiable = Model-supplied content and earlier chat history cannot be used as a basis for knowledge-base writes.

# ── Live captions (PRD-04) ─────────────────────────────────────────────────
caption-empty = There are currently no captions to save.

# ── Clipboard (PRD-01) ─────────────────────────────────────────────────────
clipboard-unavailable = The clipboard is in use by another program. Try again in a moment.

# ── Voice quick notes (PRD-02) ─────────────────────────────────────────────
voice-empty-result = The recognition result is empty and cannot be saved as a quick note.

# ── Knowledge map (PRD-16) ─────────────────────────────────────────────────
knowledge-map-no-topics = There are no topic pages, so no knowledge map is required.
knowledge-map-conflict = Multiple knowledge maps were detected. Choose which one to keep first.
knowledge-map-invalid = The knowledge map identifier is not valid.
knowledge-map-review-stale-ownership = The maintenance state of the knowledge map has changed. Please review it again.
knowledge-map-review-stale-body = The content of the knowledge map has changed. Please review it again.
knowledge-map-conflict-stale = The conflict state of the knowledge map has changed. Please check it again.

# ── Deep knowledge checkup (PRD-16) ────────────────────────────────────────
checkup-not-ignorable = This notice clears itself once the related items are handled; it does not need to be dismissed.
checkup-nothing-to-do = There are no sourced topic-page claims to verify, and the semantic index is not ready.
checkup-busy = A deep checkup is in progress. Wait for it to finish.
checkup-parse = The model output could not be parsed into a verification result.
checkup-cancelled = This deep checkup was cancelled.
checkup-no-result = The AI engine returned no result. Please try again.

# ── AI relation tags and full-library rescan (PRD-16) ──────────────────────
graph-tags-empty = The note body is empty, so no tags can be extracted.
graph-tags-busy = Relation tags for this note are being generated. Try again shortly.
graph-tags-cancelled = Tag generation was cancelled.
graph-tags-no-result = The AI engine returned no result. Please try again.
graph-tags-parse = The model output could not be parsed into a tag list.
graph-tags-vault-switched = The vault changed during generation, so the result was not saved.
graph-tags-stale = The note was modified during generation. Please try again.
graph-rescan-empty = There are no notes that require rescanning.
graph-rescan-busy = A tag rescan is in progress. Wait for it to finish.

# ── Knowledge notes and batch distillation (PRD-06-054 / PRD-13) ───────────
distill-ineligible = Only recordings, transcripts, chats and captured notes can produce a knowledge note.
distill-edited = This knowledge note has been edited manually and will not be overwritten automatically.
distill-empty-source = This material has no body content to distill.
distill-busy = A knowledge note for this note is being generated. Try again shortly.
distill-vault-switched = The vault changed during generation, so the result was not saved.
distill-no-preview = There is no preview to apply. Generate it again.
distill-template-limit = The custom template limit has been reached.
distill-batch-empty = The selected range contains no material to distill.
distill-empty = The model produced no content. Please try again.
distill-job-payload = The AI engine returned a non-text result, which cannot be used for distillation.
distill-job-no-result = The AI engine returned no result. Please try again.
distill-cancelled = Distillation was cancelled.
promote-not-quick-note = Only voice quick notes can be promoted to a note.
demote-not-note = Only a regular note can be turned back into a quick note; source material, topic pages and the knowledge map cannot.

# ── Ingesting into topic pages and synthesis (PRD-16) ──────────────────────
ingest-empty = The model produced no content. Please try again.
ingest-cancelled = The ingest was cancelled.
ingest-no-result = The AI engine returned no result. Please try again.
ingest-ineligible-not-markdown = Only Markdown notes can be ingested into a topic page.
ingest-ineligible-material = Distill this material into a knowledge note before ingesting it into a topic page.
ingest-ineligible-topic = A topic page cannot be used as an ingest source.
ingest-busy = An ingest plan for this note is being generated in the background. Try again shortly.
ingest-vault-switched = The vault changed in the meantime, so the result was not saved.
ingest-vault-switched-retry = The vault changed in the meantime. Please start over.
ingest-no-plan = There is no ingest plan waiting to be applied.
ingest-suggestion-stale = This ingest suggestion is no longer valid.
ingest-stale = The ingest plan does not match the current version of the note. Generate it again.
ingest-source-changed = The source note changed. Generate the ingest plan again.
ingest-nothing-selected = Select at least one page before applying.
ingest-not-topic = The target is not a topic page.
topic-no-sources = This tag has no notes that can be synthesised into a topic page.
topic-draft-budget = This topic preview exceeds the processing limit. Select fewer sources and try again.
topic-draft-incomplete = The model stopped before completing the topic preview. No preview was kept; try a different model or fewer sources.
topic-draft-invalid-output = The model did not return a verifiable topic preview. No content was saved.

# ── Semantic index (PRD-16) ────────────────────────────────────────────────
embed-assignment-changed = The embedding model configuration changed, so the result was not saved.
embed-shape-chunks-missing = The chunk vectors are incomplete, so this note was not added to the index.
embed-vault-switched = The vault changed in the meantime, so the result was not saved.
embed-profile-mismatch = The semantic index identifier does not match the model profile.
embed-no-model = No embedding model has been configured.
embed-nothing-to-do = Every note’s semantic index is already up to date; no rebuild is required.
embed-busy = The semantic index is being rebuilt. Wait for the current task to finish.
embed-cancelled = The semantic index rebuild was cancelled.
embed-no-result = The AI engine returned no result. Please try again.
# 🔴 One sentence covers all five contract violations: dimension, non-finite value, zero
# norm, count and payload type are the same event to the user; the difference only matters
# for diagnosis (shape facts go to the log, the stable code to the error prefix).
embed-vector-contract = The AI engine returned vectors that do not meet the expected contract, so this batch was not added to the index. Confirm that the embedding model matches the current index, and rebuild the index if necessary.

# ── System file dialogs (shared across modules) ────────────────────────────
# 🔴 One string, not one per call site (save / folder / file picker): to the user this is
# a single event — they cancelled, and nothing happened. There used to be six wordings for
# it, which is what one thing written six times looks like.
dialog-cancelled = Nothing was selected; the operation was cancelled.

# 批次 J：IPC 错误说明迁出硬编码中文（ADR-106）。
glossary-empty-field = Both the original term and its correction are required.
metrics-range-start-after-end = The start date cannot be later than the end date.
metrics-range-format = Dates must use the YYYY-MM-DD format.
metrics-unavailable = Local knowledge metrics are not ready yet. Try again shortly.
metrics-vault-unavailable = No vault is open.
workspace-metrics-clear-failed = Usage metrics are not initialized, so there is nothing to clear.
workspace-metrics-unavailable = Usage metrics are not ready yet. Try again shortly.
model-assignment-apply-failed = The assignment was not applied: refreshing the compute provider failed, and the previous assignment was restored ({ $error }).
model-assignment-not-ready = Slot “{ $slot }” requires the { $capability } capability, but the selected target is currently { $readiness }.
model-assignment-rollback-failed = The assignment was not applied and the previous one could not be restored (refresh: { $error }; rollback: { $rollback }). Set it again under Compute and Models.
model-name-empty = The model name cannot be empty.
model-delete-server-managed = This model is managed by the server and cannot be deleted locally. Manage it in the server console.
model-install-unsupported = Installing this model is not supported: { $model }.
model-install-server-managed = This model is managed by the server; installing it locally is neither needed nor possible.
model-plan-not-found-delete = That delete plan no longer exists or has already been used. Start again.
model-plan-not-found-install = That install plan no longer exists or has already been used. Start again.
model-plan-stale-install = The model, available disk space, or privacy setting has changed. Confirm again.
model-plan-stale-assignment = The model or its assignment has changed. Confirm again.
strict-local-blocked-install = Strict local mode is on, so the network install was blocked. Turn it off under Storage and Privacy, then retry.
tidy-list-stale = This note changed after the preview. Start the tidy-up again.
export-no-transcript-batch = None of the selected notes have transcript content to export.
export-no-text-batch = None of the selected notes have body text to export.
export-no-transcript-single = This note has no transcript content to export yet.
export-write-failed = No files were written. Check that the destination folder is writable.
diarize-not-applicable-audio = This recording has no audio segments, so speaker separation cannot run.
diarize-not-applicable-media = This recording has no playable media segments.
diarize-no-transcript = This recording has no transcript yet. Transcribe it first.
diarize-models-missing = The speaker separation models are not installed. Download both of them under Settings → Models → Speaker separation, then try again.
knowledge-lifecycle-stale = The target note is gone or its body changed, so this knowledge update was discarded.
knowledge-lifecycle-vault-switched = The vault was switched, so the earlier knowledge update is no longer valid.
knowledge-apply-stale = The sources, topic, or scope of this update plan changed, or you rejected it, so the topic was not written. Generate a new plan.
knowledge-apply-unavailable = This topic update plan could not be applied right now. The topic was not changed; the plan is kept so you can retry later.
source-grant-unavailable = This permission could not be saved, so recordings and transcripts will not be used as sources yet. Try again later.
topic-upgrade-batch = Upgrade at most { $max } topic pages at a time; please select them in batches.
translate-no-transcript = This recording has no transcript yet. Transcribe it first.
backend-i18n-unavailable = Backend copy resources are not initialized.
fetch-media-already-present = This transcript already has a local media file.
fetch-no-source-url = This transcript has no source URL on record.
trust-cloud-unavailable = This build does not include the cloud client.
empty-result-note = The result is empty; there is nothing to save into a note.
empty-result-lens-note = The result is empty; there is nothing to save into a Lens note.
vault-missing-path-gone = The vault path no longer exists: { $path }
vault-missing-open-failed = The vault could not be opened: { $error }
vault-in-cloud = The files of the vault at { $path } are still in iCloud. Lunarca has asked iCloud to download them to this Mac; iCloud downloads can be slow, so try again later, or right-click the folder in Finder and choose “Keep Downloaded”.
vault-switch-busy = Some tasks could not be paused in time, so the switch was cancelled. Try again shortly.
vault-queue-save-failed = Pending tasks could not be saved to the vault, so the switch was cancelled. Check disk space and folder permissions, then try again.
vault-switch-recording = A recording is in progress. Stop it before switching vaults.
empty-asset-download = The downloaded content is empty.
empty-asset-paste = The pasted content is empty.
invalid-data-url = Only base64-encoded data URLs are supported.
invalid-note-path = That note path is not valid.
note-not-found = The note does not exist or is outside the vault: { $path }
not-an-image = The response is not an image ({ $mime }).
not-a-note = Only notes can be opened in a separate window.
window-failed = Could not open the sticky note window: { $error }
route-rejected = Unsupported navigation target: { $route }
strict-local-blocked-image = Strict local mode is on, so downloading the remote image was blocked. Turn it off under Storage and Privacy, then retry.
url-rejected = Only http/https image links are supported.
vault-not-registered = That folder is not in your vault list. Add it under Manage vaults first.
vault-window-is-active = This vault is already open in the main window, so a separate window is not needed.
app-restarting = Lunarca is saving and restarting. Try opening the window again in a moment.

# 批次 J′：够不着 AppHandle 的那一批（ADR-107）。
vault-switching = Switching vaults. Try again in a moment.
vault-not-open = No vault is selected. Choose or create one in the setup guide or in Settings.
workspace-metrics-range-start-after-end = The start date cannot be later than the end date.
workspace-metrics-range-format = Dates must use the YYYY-MM-DD format in UTC.
vault-window-gone = That secondary vault window is no longer valid. Open it again.
setting-domain-control-not-a-slot = { $key } is not a model assignment slot.
setting-domain-control-assign = Model assignments must be made through Assign model or Clear assignment.
setting-domain-control-clear = Clearing a model assignment must be done through Clear assignment.
empty-note = The note is empty and cannot be saved.
empty-quick-note = The quick note is empty and cannot be saved.
vault-create-exhausted = A unique note path could not be generated. Use a different name and try again.
model-load-failed-retranscribe = The re-transcription model failed to load: { $error }
model-load-failed-asr = The transcription model failed to load. Reinstall it, or choose a valid whisper weight in Settings: { $error }
model-missing-realtime-asr = No ready real-time transcription model was found. Install and assign one under Settings → Models.
remote-asr-disabled-realtime = Real-time transcription points at the server, but server-side transcription is turned off. Enable it under Settings → Compute and Models.
remote-asr-disabled-batch = Batch transcription points at the server, but server-side transcription is turned off. Enable it under Settings → Compute and Models.
path-rejected-file = The file does not exist or is outside the vault: { $path }
path-rejected-entry = The item does not exist or is outside the vault: { $path }
url-rejected-external = Only http/https external links are supported: { $url }
empty-instruction = The custom instruction cannot be empty.
invalid-name-empty = The name cannot be empty.
invalid-name-separator = The name cannot contain path separators or control characters.
invalid-name-generic = That name is not valid.
invalid-move-into-self = A folder cannot be moved inside itself.
move-target-exists = The destination folder already holds something with that name, and moving would overwrite it. Rename it or pick another place.
vault-name-empty = The vault name cannot be empty.
vault-nested = This folder overlaps the existing vault “{ $name }”; their indexes and file watchers would collide. Choose a separate folder.
vault-path-invalid-parent-component = The path for a new vault cannot contain unresolved parent components (`..`).
vault-path-invalid = The vault path could not be resolved: { $detail }
strict-local-blocked-realtime-asr = Real-time transcription points at the server, but the current privacy mode is local-only. Change the privacy mode, or point that slot back at an on-device model.
strict-local-blocked-batch-asr = Batch transcription points at the server, but the current privacy mode is local-only. Change the privacy mode, or point that slot back at an on-device model.

# 语音速记 AI 提炼写回正文的待办小标题（PRD-06-058 C′）。
quick-note-refine-todos = To-do
note-ai-invalid = The note or action for this background AI run is no longer valid. Start it again from the note.

# Suffix for duplicated files ("Plan copy.md", V5a).
duplicate-suffix = copy

# Official sync (ADR-148): the note inside a conflict copy name, `name (conflict copy 2026-10-09).md`.
sync-conflict_copy_label = conflict copy

# Point-in-time restore (PRD-13-007): the new folder name; the time is the chosen point.
sync-restore-folder-name = { $name } (restored to { $time })
setting-reset-stale = Settings changed. Review the reset plan again.
setting-domain-control-reset = Use the dedicated control for this setting.

## Creator space: skeleton text for a new video project (ADR-162, PRD-19-002)
creator-note-research = Research
creator-note-script = Script
creator-section-idea = Idea
creator-section-idea-hint = In one sentence: what this video is about and who it is for.
creator-section-research = Research
creator-section-script = Script
creator-research-sources = Sources
creator-research-quotes = Quotes
creator-material-folder = Materials
creator-research-materials = Materials
creator-script-hook = Hook
creator-script-beat = Beat { $n }
creator-script-outro = Outro

## Creator space: publishing package (ADR-162 ⑪, PRD-19-018~020)
creator-publish-folder = Publish
creator-publish-source-outside = This final cut is not in this project's folder. Pick it again.
creator-not-a-project = This isn't a video project's home note, so the final cut can't be imported here.
creator-bad-date = The planned publish date must be written as year-month-day, for example 2026-10-24.
creator-example-title = Example - Running an LLM on a laptop
creator-channel-name = A channel name can’t be empty or longer than 40 characters.
creator-broll-file = shot list
creator-broll-col-index = No.
creator-broll-col-section = Section
creator-broll-col-shot = Shot
creator-broll-col-done = Done
creator-broll-opening = Opening
creator-broll-yes = yes
creator-project-name = The project name can't be empty or made only of characters that can't be used in file names.
creator-project-name-taken = A project or note with that name already exists. Try another name.
creator-source-text = Pasted text can't be empty or longer than 200,000 characters. Save longer text as a file and import it instead.
creator-pasted-title = Pasted text
creator-research-empty = This source has no readable text to work from.
creator-research-pick = Pick between 1 and { $max } sources.
creator-brief-file = Research brief
creator-brief-based-on = Based on: 
creator-brief-separator = , 
creator-brief-overview = Overview
creator-brief-findings = Findings
creator-brief-questions = Open questions
creator-research-no-brief = This project has no research brief yet. Write one from Materials & sources first.
creator-topic-text = The topic can’t be empty or longer than 500 characters.
creator-script-draft = Script first pass
link-import-rights-unconfirmed = Before importing from a link for the first time, confirm that you only import content you have the right to use.
creator-channel-youtube = YouTube
creator-channel-bilibili = Bilibili
creator-channel-youtube_shorts = YouTube Shorts
creator-channel-douyin = Douyin
creator-channel-kuaishou = Kuaishou
creator-channel-weixin_channels = WeChat Channels
creator-channel-xiaohongshu_video = Xiaohongshu video
creator-channel-tiktok = TikTok
creator-channel-instagram_reels = Instagram Reels
creator-channel-wechat_article = WeChat article
creator-channel-xiaohongshu_note = Xiaohongshu note
creator-channel-weibo = Weibo
creator-channel-x = X
creator-channel-linkedin = LinkedIn
creator-channel-newsletter = Email newsletter
creator-channel-podcast = Podcast
creator-publish-package = { $channel } package
creator-publish-description = Description
creator-channel-unknown = That channel is no longer in the channel list. Refresh the Publish tab and try again.
creator-distribute-blog = Blog
creator-distribute-newsletter = Newsletter
creator-distribute-thread = Thread
creator-distribute-shorts = Shorts
creator-distribute-from-script = script
creator-distribute-from-transcript = transcript of the final cut “{ $title }”
creator-distribute-derived = AI draft rewritten from the { $source } of { $project }. Read it through before you publish.
creator-distribute-clip-range = In { $start } · Out { $end }
creator-distribute-needs-cut = Picking short clips needs the final cut’s transcript (it has timestamps). Import the final cut on the Publish tab first.
creator-distribute-no-script = This project has no script yet. You can rewrite from the final cut’s transcript instead.
creator-swipe-snippet = Snippet
creator-swipe-why = Why it works
creator-swipe-source = Source
creator-swipe-pattern-prefix = pattern
creator-swipe-too-few = You need at least { $min } swipes to group them.
creator-swipe-bad-name = “{ $name }” can't be used as a tag. Try another name.
creator-quote-dash = { " — " }
app-disabled = This belongs to an app that isn't installed or is turned off. Install or turn it on in Apps; nothing in your vault is affected.
app-not-installed = This app isn't installed yet. Install it in Apps first.
creator-quote-empty = The selection has no text once timestamps are removed, so it can't be saved as a quote.
creator-quote-no-project = This note doesn't belong to any video project (it isn't in a project folder and isn't linked with a project's home note), so there's nowhere to save the quote.
creator-publish-chapters = Chapters
creator-chapters-parse = The AI did not return usable chapters this time. Try again.
creator-publish-parse = The AI did not return a usable result this time. Try again.
creator-publish-links = Links
creator-publish-titles = Title options
creator-publish-cover = Cover text
creator-publish-category = Section
creator-publish-tags = Tags
creator-publish-sources = Sources

## Document import (ADR-164)
import-filter-materials = Audio, video and documents
doc-import-notice = Imported from { $file }
doc-import-undecodable = Some characters could not be decoded and show as �.
doc-import-images-omitted = Some images were not brought in; only their alt text is kept.
doc-import-table-flattened = Merged cells or nested tables were flattened.
doc-import-comments = Comments were not brought in.
doc-import-tracked-changes = Tracked changes were not brought in: deleted text is dropped, inserted text is kept.
doc-import-text-boxes = Text inside text boxes was not brought in.
doc-import-notes = Footnotes and endnotes were not brought in.
doc-import-table-truncated = The table is { $rows } × { $cols } (rows × columns); only the first { $shown_rows } × { $shown_cols } are shown here. The full data is in the original file.
doc-import-unsupported = This kind of file can't be imported as a document yet.
doc-import-too-large = The file is larger than 100 MB and was not imported.
doc-import-empty = There is no text in the file to import.
doc-import-corrupt = The file can't be opened: it may be damaged, or not the format its extension says.
doc-import-sheet-truncated = Sheet “{ $sheet }” is { $rows } × { $cols } (rows × columns); only the first { $shown_rows } × { $shown_cols } are shown here. The full data is in the original file.
doc-import-needs-macos = Legacy Word (.doc), RTF and ODT can only be imported on macOS for now. Save the file as .docx in Word and import that instead.
doc-import-timeout = Converting this file took more than a minute and was stopped; the file may be damaged. Save it as .docx in Word and import that instead.
doc-import-page = Page { $page }
doc-import-page-separator = ,{" "}
doc-import-pages-without-text = Pages { $pages } have no extractable text (they may be images or scans).
doc-import-pages-unreadable = Pages from { $from } on (of { $total }) could not be read and were not brought in.
doc-import-encrypted = This PDF is password-protected and was not imported. Remove the password (for example in Preview) and import it again.
doc-import-no-text-layer = This PDF has almost no extractable text and looks like a scan. Recognizing text in images isn't supported yet, so it was not imported.
creator-quote-page = p. { $page }
doc-import-read-timeout = Reading this file timed out: it may still be in cloud storage (iCloud, OneDrive, …) and not downloaded to this Mac. Open or download it in Finder first, then retry.
doc-import-read-failed = This file can't be read: it may have been moved or deleted, or there's no permission to read it.
doc-import-mostly-images = This PDF has very little text; its content is probably mostly images (for example, a web page printed from a browser). Text inside images was not brought in.

## Web clipping (ADR-164 ⑦)
strict-local-blocked-web-clip = Strict local mode is on, so web clipping is blocked. Turn it off under Storage & Privacy and try again.
web-clip-disabled = Web clipping is turned off in Settings (Storage & Privacy ▸ Web clipping). Turn it on and try again.
web-clip-timeout = The page didn't respond within 30 seconds and was not clipped. Try again later.
web-clip-fetch-failed = This page can't be opened: there's no connection, or the site refused the request.
web-clip-http = The site returned an error (HTTP { $status }); nothing was clipped.
web-clip-not-html = This link isn't a web page (it may be a PDF, image or download). Download it and import it with Import material….
web-clip-too-large = The page is larger than 10 MB and was not clipped.
web-clip-no-article = No article text could be found on this page (it may need a login, or load its content with scripts). Save it from your browser as a web page and import it with Import material….
web-clip-notice = Clipped from { $source }
web-clip-author = Author: { $author }
web-clip-published = Published: { $published }
web-clip-snapshot = Full-page snapshot at the time: { $file }
