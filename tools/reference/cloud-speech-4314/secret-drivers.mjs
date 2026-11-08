import * as Effect from 'effect/Effect';
import * as Deferred from 'effect/Deferred';
import {definition} from 'file:///native/packages/secret-provider-onepassword/src/shared/definition.ts';
export let state;
export const reset=value=>{state=value;};
export const BUILT_IN_SECRET_PROVIDER_DRIVER_LIST=[{kind:definition.kind,definition,read:(config,references)=>Effect.gen(function*(){
 state.events.push({kind:'driver-read',account:config.account,references});state.entered=true;
 if(state.gate)yield* Deferred.await(state.gate);
 return references.map(()=>state.byAccount[config.account]);
})}];
