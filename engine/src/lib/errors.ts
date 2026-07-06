/** An error whose message is meant for humans; the CLI prints it without a stack trace. */
export class PreceiptsError extends Error {
  override readonly name = "PreceiptsError";
  readonly exitCode: number;

  constructor(message: string, opts: { exitCode?: number } = {}) {
    super(message);
    this.exitCode = opts.exitCode ?? 1;
  }
}
