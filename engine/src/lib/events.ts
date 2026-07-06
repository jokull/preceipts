/**
 * NDJSON events emitted by `preceipts run --events` and delivered to library
 * consumers (the future cockpit TUI) as the same objects.
 */

export interface PrepareStartedEvent {
  event: "prepare-started";
  step: string;
  ts: string;
}

export interface PrepareOutputEvent {
  event: "prepare-output";
  step: string;
  chunk: string;
}

export interface PrepareFinishedEvent {
  event: "prepare-finished";
  step: string;
  ok: boolean;
  exit: number;
  duration_ms: number;
}

/** Prepare changed the worktree; receipts will key to the normalized tree. */
export interface TreeNormalizedEvent {
  event: "tree-normalized";
  tree: string;
  changed: string[];
}

/** Checks are about to run against this tree — the tree receipts will mint on. */
export interface RunStartedEvent {
  event: "run-started";
  tree: string;
  dirty: boolean;
}

/**
 * The worktree changed while checks ran (a check mutated it, or files were
 * edited mid-run). The run is invalid: no receipts are minted.
 */
export interface WorktreeChangedEvent {
  event: "worktree-changed";
  before: string;
  after: string;
  changed: string[];
}

export interface CheckStartedEvent {
  event: "check-started";
  check: string;
  tree: string;
  ts: string;
}

export interface OutputEvent {
  event: "output";
  check: string;
  chunk: string;
}

export interface CheckFinishedEvent {
  event: "check-finished";
  check: string;
  ok: boolean;
  exit: number;
  duration_ms: number;
}

export interface ReceiptMintedEvent {
  event: "receipt-minted";
  check: string;
  tree: string;
  /** "blob:<sha>" reference to the stored log. */
  log: string;
}

export type RunEvent =
  | PrepareStartedEvent
  | PrepareOutputEvent
  | PrepareFinishedEvent
  | TreeNormalizedEvent
  | RunStartedEvent
  | WorktreeChangedEvent
  | CheckStartedEvent
  | OutputEvent
  | CheckFinishedEvent
  | ReceiptMintedEvent;

export type RunEventHandler = (event: RunEvent) => void;
