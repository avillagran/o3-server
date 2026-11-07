import * as Context from 'effect/Context';
export class EnvironmentAuth extends Context.Service()('fixture/http/EnvironmentAuth'){}
export class SessionStore extends Context.Service()('fixture/http/SessionStore'){}
export class CloudSpeech extends Context.Service()('fixture/http/CloudSpeech'){}
export class TextGeneration extends Context.Service()('fixture/http/TextGeneration'){}
export const verifyRequestDpopProof=()=>{throw Error('No cryptographic authentication requested by this principal-injected native HTTP fixture');};
