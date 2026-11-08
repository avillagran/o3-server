// Compose the unchanged real-owner acceptance with an explicit CDP producer.
import fs from 'node:fs';import assert from 'node:assert/strict';
await import('./owned-gateway.mjs');
const proof=JSON.parse(fs.readFileSync('/out/native-page-worker/capacity-proof.json','utf8'));
assert.equal(proof.complete,true);assert.equal(proof.fixtureBackgroundProducers,7);assert.equal(proof.actualBrowserEvaluationsStarted,8);
assert.equal(proof.terminationWireOutstanding,9);assert(proof.maximumRealGatewayOutstanding>=9);assert.equal(proof.fixtureMaximumGatewayOutstanding,9);assert.deepEqual(proof.browserOrder,[0,1,2,3,4,5,6,7]);assert.equal(proof.diagnosticHealthy,true);
assert.equal(proof.wireOutstandingAtMarkerAck,0);assert.equal(proof.originalReceiptAndSessionPreserved,true);
assert.equal(proof.originalNativeTailDidNotWaitForTerminationDelivery,true);
fs.writeFileSync('/out/capacity-proof.json',JSON.stringify(proof,null,2)+'\n');
console.log('ACTUAL_GATEWAY_NINE_WIRE_CALLS_PASS');
