/**
 * NDJSON events emitted by `preceipts run --events` and delivered to library
 * consumers (the future cockpit TUI) as the same objects.
 */

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

export type RunEvent = CheckStartedEvent | OutputEvent | CheckFinishedEvent | ReceiptMintedEvent;

export type RunEventHandler = (event: RunEvent) => void;
