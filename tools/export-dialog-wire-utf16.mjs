// Native JavaScript JSON.parse/stringify and toWellFormed are the wire oracle.
import { writeFileSync } from "node:fs";
const output = process.argv[2];
if (!output) throw new Error("An explicit output path is required");
const seeds = ["", "plain", "😀", "\ud800", "\udc00", "a\ud800b", "\ud800\ud800\udc00\udc00", "\0\b\f\n\r\t", '\\"/🧪', "\\ud800"];
for (let index = 0; index < 128; index++) {
  seeds.push(String.fromCharCode(0xd7ff + index * 19, 0xdc00 + index * 7, 0x20 + index));
}
const wire = (message, prompt) => JSON.stringify({sessionId:"owned",method:"Page.javascriptDialogOpening",params:{type:"prompt",message,defaultPrompt:prompt,url:"https://example.test/"}});
const rows = seeds.map((text, index) => wire(text, seeds[(index * 17 + 3) % seeds.length]));
rows.push('{"method":"ignored","method":"Page.javascriptDialogOpening","params":{"message":"discarded"},"params":{"message":"\\ud800","message":"\\udc00","type":"alert"},"sessionId":"owned"}');
rows.push('{"method":"Page.javascriptDialogOpening","params":{"message":"\\ud800","message":false,"defaultPrompt":"\\udc00","defaultPrompt":null,"type":"prompt"}}');
const unitArray = value => typeof value === "string" ? Array.from({length:value.length}, (_,index) => value.charCodeAt(index)) : null;
const cases = rows.map(raw => {
  const parsed = JSON.parse(raw);
  return {raw,message:unitArray(parsed.params.message),defaultPrompt:unitArray(parsed.params.defaultPrompt),compat:JSON.stringify(parsed,(_,value)=>typeof value==="string"?value.toWellFormed():value)};
});
writeFileSync(output, JSON.stringify({reference:"T3 nightly4301 CDP JSON wire; native JavaScript string semantics",cases},null,2)+"\n",{flag:"wx"});
