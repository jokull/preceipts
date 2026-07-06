export { PreceiptsError } from "./errors.ts";
export { git, gitOut, repoRoot, revParse, hasRemote, catFileBatch, hashObjectWrite } from "./git.ts";
export type { GitResult, GitOptions } from "./git.ts";
export { parseDuration, formatDuration } from "./duration.ts";
export { capLog, LOG_HEAD_CAP, LOG_TOTAL_CAP } from "./log-cap.ts";
export {
  encodeReceipt,
  parseReceiptLine,
  parseReceipts,
  latestByCheck,
  logBlobSha,
} from "./receipt.ts";
export type { Receipt, Runner } from "./receipt.ts";
export {
  loadConfig,
  listChecks,
  PRECEIPTS_DIR,
  CHECKS_DIR,
  CONFIG_FILE,
  DEFAULT_TIMEOUT_MS,
} from "./config.ts";
export type { Config, CheckFile, PrepareStep } from "./config.ts";
export { computeWorkingTree } from "./tree-hash.ts";
export type { WorkingTree } from "./tree-hash.ts";
export {
  NOTES_REF,
  NOTES_REMOTE_TRACKING_REF,
  LOG_REF_PREFIX,
  appendReceiptLine,
  readNote,
  readReceipts,
  readAllReceipts,
  storeLog,
  readLog,
} from "./notes.ts";
export type {
  RunEvent,
  RunEventHandler,
  PrepareStartedEvent,
  PrepareOutputEvent,
  PrepareFinishedEvent,
  TreeNormalizedEvent,
  RunStartedEvent,
  WorktreeChangedEvent,
  CheckStartedEvent,
  OutputEvent,
  CheckFinishedEvent,
  ReceiptMintedEvent,
} from "./events.ts";
export { runChecks } from "./runner.ts";
export type { RunOptions, RunResult, RunInvalidated } from "./runner.ts";
export { computeStatus, resolveTree } from "./status.ts";
export type { Status, StatusRow, StatusOptions, CheckState } from "./status.ts";
export { computeHud } from "./hud.ts";
export type { Hud, HudOptions } from "./hud.ts";
export { land, buildTrailers } from "./land.ts";
export type { LandOptions, LandResult, StaleInfo } from "./land.ts";
export { sync } from "./sync.ts";
export type { SyncResult } from "./sync.ts";
export { gc, DEFAULT_KEEP_SUCCESS_MS, DEFAULT_KEEP_FAILURE_MS } from "./gc.ts";
export type { GcOptions, GcResult } from "./gc.ts";
export { init, PUSH_REFSPECS, FETCH_REFSPECS } from "./init.ts";
export type { InitOptions, InitResult } from "./init.ts";
