// Shared collaboration pages (P06–P14). Transport-agnostic: Web and Desktop
// supply their own command transport and projection loaders.
export { ProjectFiles } from "./ProjectFiles";
export { ProjectHistory, CaptureDetail } from "./ProjectHistory";
export { ComparePanel } from "./ComparePanel";
export { ReviewList } from "./ReviewList";
export type { ReviewFilter } from "./ReviewList";
export { ReviewDetail } from "./ReviewDetail";
export type { ReviewTab } from "./ReviewDetail";
export { ReviewValidations, RunDetail } from "./ReviewRuns";
export { ProposalForm } from "./ProposalForm";
export { Releases } from "./Releases";
export type { ReleasesTab } from "./Releases";
export { useScopedTasks } from "./useScopedTasks";
export {
  captureNames,
  captureTitle,
  decisionSummary,
  denied,
  emptyHistoryFilters,
  emptyReferences,
  explainCollabError,
  folderChildren,
  historyFilterRequest,
  localDayBoundary,
  namedCaptures,
  personLabel,
  projectReferences,
  reviewLifecycle,
  searchFiles,
} from "./model";
export type { CollabErrorInfo, FileRow, HistoryFilters } from "./model";
export type {
  CollabCapabilities,
  CollabCommand,
  CollabTransport,
  PeopleDirectory,
  ProjectReferences,
  ReviewDetailData,
  ReviewProposal,
  ReviewSummary,
  RunDetailData,
  RunSummary,
} from "./types";
