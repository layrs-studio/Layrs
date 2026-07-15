export class DesktopOperationCancelledError extends Error {
  constructor() {
    super("Desktop operation was cancelled.");
    this.name = "DesktopOperationCancelledError";
  }
}

export interface DesktopOperationOptions<T> {
  busyAction?: string;
  coalesce?: boolean;
  key: string;
  task: () => Promise<T>;
}

export class DesktopOperationController {
  private coalesced = new Map<string, Promise<unknown>>();
  private generation = 0;
  private nextOwner = 0;
  private owner: number | null = null;
  private tail: Promise<void> = Promise.resolve();

  constructor(private readonly onBusyChange: (busyAction: string | null) => void) {}

  run<T>({ busyAction, coalesce = false, key, task }: DesktopOperationOptions<T>): Promise<T> {
    const existing = coalesce ? this.coalesced.get(key) : undefined;
    if (existing) {
      return existing as Promise<T>;
    }

    const scheduledGeneration = this.generation;
    const owner = ++this.nextOwner;
    const execute = async () => {
      if (scheduledGeneration !== this.generation) {
        throw new DesktopOperationCancelledError();
      }
      if (busyAction) {
        this.owner = owner;
        this.onBusyChange(busyAction);
      }
      try {
        const result = await task();
        if (scheduledGeneration !== this.generation) {
          throw new DesktopOperationCancelledError();
        }
        return result;
      } finally {
        if (busyAction && this.owner === owner) {
          this.owner = null;
          this.onBusyChange(null);
        }
      }
    };

    const promise = this.tail.then(execute, execute);
    this.tail = promise.then(
      () => undefined,
      () => undefined
    );
    if (coalesce) {
      this.coalesced.set(key, promise);
      void promise.finally(() => {
        if (this.coalesced.get(key) === promise) {
          this.coalesced.delete(key);
        }
      }).catch(() => undefined);
    }
    return promise;
  }

  invalidate() {
    this.generation += 1;
    this.coalesced.clear();
    if (this.owner !== null) {
      this.owner = null;
      this.onBusyChange(null);
    }
  }
}

export function isDesktopOperationCancelled(error: unknown): boolean {
  return error instanceof DesktopOperationCancelledError;
}
