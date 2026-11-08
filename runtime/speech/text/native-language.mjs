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
/**
 * transcribe.cpp accepts a language only when it equals an entry of the GGUF's
 * `general.languages` byte for byte. Nemotron Streaming 3.5 lists `de-DE` where the catalog
 * says `de`, so the request carries the loaded model's own spelling. "auto" omits the hint.
 */
export function nativeSpeechLanguage(
  advertised                   ,
  language        ,
)                     {
  if (language === "auto") return undefined;
  const tag = (value        ) => value.toLowerCase().replaceAll("_", "-");
  const wanted = tag(language);
  return (
    advertised.find((entry) => entry === language) ??
    advertised.find((entry) => tag(entry) === wanted) ??
    (wanted.includes("-")
      ? undefined
      : advertised.find((entry) => tag(entry).split("-", 1)[0] === wanted)) ??
    language
  );
}
