import { NativeReviewSurface, NativeReleaseSurface, NativeHistorySurface } from "@layrs/ui";
import { nativeRemote } from "./tauri";
export function NativeReviewPanel({root,spaceId,origin,principal,selectedReview,ca,busy,onBusy}:{root:string;spaceId?:string;origin:string;principal:string;selectedReview?:{reviewId:string;version:number}|null;ca:string;busy:boolean;onBusy:(busy:boolean)=>void}) {
  const transport = <T,>(command:{action:string;[key:string]:unknown})=>nativeRemote<T>(root,{origin,caPem:ca||null,action:{action:"collaboration",command}});
  return <><NativeReviewSurface spaceId={spaceId} selectedReview={selectedReview} draftIdentity={principal ? {origin,principalId:principal} : undefined} scope={root+"|"+origin+"|"+principal} busy={busy || !origin} onBusy={onBusy} transport={transport} /><NativeReleaseSurface scope={root+"|"+origin} busy={busy || !origin} onBusy={onBusy} transport={transport} /><NativeHistorySurface scope={root+"|"+origin+"|"+principal} busy={busy || !origin || !principal} onBusy={onBusy} transport={transport} /></>;
}
