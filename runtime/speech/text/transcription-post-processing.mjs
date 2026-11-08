/*
MIT License

Copyright (c) 2026 T3 Tools Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

Source: T3 Code nightly4314 83a3674ffd84ffb33de768210c806cb1ac53fd80
Complete original module after type erasure; model catalog dependency embedded.
*/
const quoteCue = (cue        ) => JSON.stringify(cue);

/** Builds the cleanup prompt: instructions, the transcript, and the draft around the insertion point. */
export function buildTranscriptionPostProcessingPrompt(
  instructions        ,
  transcript        ,
  draft   
                          
                                                                         
   ,
  correctionCue = "",
)         {
  const cue = correctionCue.trim();
  const cueInstructions = cue
    ? `\n\nCorrection cue: ${quoteCue(cue)}. Only when this cue clearly marks a spoken self-correction, apply the correction the speaker made and omit the cue from the result. The correction may revise, add to, or retract earlier speech. Preserve everything else. If the cue is an intended part of the sentence, keep it. Use the surrounding context to decide; do not assume every occurrence is a correction.`
    : "";
  const cursor = Math.max(0, Math.min(draft.selection.start, draft.text.length));
  const selectionEnd = Math.max(cursor, Math.min(draft.selection.end, draft.text.length));
  const replacesSelection = selectionEnd > cursor;
  const placement = replacesSelection
    ? "The transcript will replace composer_selection between composer_before and composer_after. Use the selected text only as context for terminology and references; it will not remain in the draft. Do not preserve or copy it into the result unless the new speech itself says it."
    : "The transcript will be inserted at the cursor between composer_before and composer_after.";
  const selectionBlock = replacesSelection
    ? `\n<composer_selection>\n${draft.text.slice(cursor, selectionEnd)}\n</composer_selection>`
    : "";
  return `${instructions}${cueInstructions}

<transcript>
${transcript}
</transcript>

${placement} Use the composer draft and insertion position to resolve ambiguity and correct likely recognition errors in the transcript. The draft can clarify the topic, references, terminology, and how the new speech fits with nearby text. Prefer an interpretation supported by the draft when the speech plausibly matches it. Do not invent content, change clear speech, or include existing composer text in the result. Return only the cleaned transcript.
<composer_before>
${draft.text.slice(0, cursor)}
</composer_before>${selectionBlock}
<composer_after>
${draft.text.slice(selectionEnd)}
</composer_after>`;
}
