# Speech settings and private credentials foundation

Reference: T3 Code nightly4314, `83a3674ffd84ffb33de768210c806cb1ac53fd80`.

The active closed settings codec is generated from the complete pinned contracts
and managed-settings graph. Its native oracle covers 650 schema nodes, 14
transforms, 1,204 complete-setting cases, 18,200 node cases and 563 managed
settings scenarios. This replaces the earlier default adapter with the actual
4314 schema, including the seven-day automatic-settlement default.

The native secret-field inventory adds the cloud speech credential group after
the existing source-control, usage and device groups. OpenAI, xAI and OpenRouter
credentials use the exact `voice-cloud-openai`, `voice-cloud-xai` and
`voice-cloud-openrouter` private slots. Public settings contain saved markers;
materialization restores owned private values under the existing settings lock.
The physical bridge includes the group during read, write, rollback, redaction
and provider-reference cleanup. Unrelated settings updates retain credentials.
Clearing one credential affects only its known owned slot.

The whole native secret oracle covers 172 mapping cases, 40 resolution cases,
15 host decoding cases, 27 transaction cases, 860 planning observations, eight
apply observations, 516 materialization cases, 170 materialization error cases
and 517 profile projections. Source files are checked against the pinned source
manifest and run with one canonical Effect 4.1.2 instance in a private container.
These are specific producer and consumer proofs, rather than whole-service
certification. Unicode case-folding follows the compiled Rust Unicode tables;
full Node Unicode 17 locale behavior and raw lone-surrogate JSON are not claimed
by these settings codecs.

This checkpoint provides settings and private storage only. It does not expose
voice capabilities, speech HTTP handlers, a transcription engine, model
downloads, microphone access, uploads or browser recording consent. Local model
lifecycle, worker ownership, PCM processing and cloud execution require their
own actual consumers and acceptance. Device and unsupported-driver private-slot
consumers retain their existing activation gates. Runtime fixtures contain only
synthetic credentials, stay private and are retained without tree deletion.
