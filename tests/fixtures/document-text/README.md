# Nightly document fixtures

`nightly-cases.json.gz` contains the 64 evaluated `cases` from the test
“preserves office structure and EPUB reading order with bounded archives” in
`apps/server/src/provider/userInputAttachments.test.ts` at T3 Code nightly4165,
commit `8b44f42b14c8e273156474a924ccf29680dd7857`.

It stores parts, ordered expected substrings, absent substrings and repeat counts.
UTF-16 byte buffers use Node's JSON `{type:"Buffer",data:[...]}` representation.
Gzip keeps the repeated size/depth/output-limit inputs small in Git. Tests generate
Stored ZIP entries, except embedded bitmap/binary media, which uses Deflate just as
upstream does. `tests/document_text.rs` drives the Rust CLI and checks the original
expectations, including all nine formats across these and the RTF/notebook cases.

These fixtures are MIT-licensed T3 Tools data; see
[the retained license notice](../../../docs/THIRD-PARTY-NOTICES.md).
