// Retain source producer accounting across interruption. A closed native Queue
// reports a successful false result; that reservation has no host consumer.
export function reserveProducer(Effect,Exit,{reserve,offer,abandon}){
 return Effect.uninterruptibleMask(restore=>Effect.tryPromise({try:reserve,catch:error=>error}).pipe(
  Effect.flatMap(()=>restore(offer).pipe(
   Effect.onExit(exit=>Exit.isFailure(exit)||exit.value===false?Effect.promise(abandon):Effect.void)
  ))
 ));
}
