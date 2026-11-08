// Foreground operation routing from the original preview toolkit handlers.
// Optional status enrichment is a separately constructed owner invocation.
const operations=Object.freeze({
 preview_status:['status'],preview_open:['open','navigate'],preview_dialog:['dialog'],
 preview_navigate:['navigate'],preview_resize:['resize'],preview_set_appearance:['setColorScheme'],
 preview_snapshot:['snapshot'],preview_act:['click','type','hover','select','drag','upload','press','scroll'],
 preview_evaluate:['evaluate'],preview_wait_for:['waitFor'],
 preview_recording_start:['recordingStart'],preview_recording_stop:['recordingStop'],
});
export function permitsPageOperation(tool,operation){
 return Object.hasOwn(operations,tool)&&operations[tool].includes(operation);
}
