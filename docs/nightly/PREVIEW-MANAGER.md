# Preview manager integration

The pinned reference is nightly 4288, commit
`32a0531748c44b25072d7495ce0b9bd7e9450a87`, with Effect 4.1.2.
`tools/generate-preview-manager-fixtures.mjs` executes the complete native
`preview/Manager.ts` and its actual contracts and shared URL helpers. Only the
clock and random service are injected. Its output contains 43 manager traces,
including revision/event ordering, server ownership, reveal, sticky headless state,
status/resize/adjust, missing sessions and insertion-ordered closure past 36 tabs.
Those manager traces are reference evidence; their coordinator integration is pending.

The URL fixture contains 99 actual helper observations and actual public open-schema
admission results. Rust compares 98 helper outputs/errors exactly and checks one
oversized input that the native public schema rejects before normalization. This
preserves JavaScript trimming, explicit non-web scheme rejection, UTF-16 input
lengths, typed protocol metadata and the complete native error message.

The underlying Rust IDNA implementation limits a Unicode label to 1,000 scalar
values. Native URL parsing can accept longer labels, including some within the
public URL length budget. Arbitrary long-label/raw-helper parity remains open;
the oversized fixture must not be represented as matching raw helper output.
Escaped lone UTF-16 surrogates remain a separate cross-runtime boundary.

The desktop consumer needs a publication-order change: the native manager publishes
the initial `opened` snapshot before backend attachment waits, with ownership keyed
by that tab before publication. Server tab IDs also include the server epoch so a
persisted client surface cannot attach to a new tab after restart. Current O3
coordination still launches its local browser before publishing. Adding an attachment
wait to that path would prevent the guest from learning its tab ID in time.

Browser bootstrap fields remain refused until manager publication/fencing, preview
consumers and the actual owned desktop launcher are connected and tested together.
