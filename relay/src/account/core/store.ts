// Where one object's state lives and when it is next swept; a Durable Object, a table row or a map.
export interface Store<T> {
  get(): Promise<T | undefined>;
  put(value: T): Promise<void>;
  erase(): Promise<void>;
  // Asks for a sweep by that time: set if none is booked or the booked one is later.
  wakeBy(at: number): Promise<void>;
  cancel(): Promise<void>;
}

// What the sign-in objects need from their surroundings.
export interface Deps {
  // The secret that keys the stored code hashes; without it no code can be sent.
  codeKey(): string | undefined;
  mailer(): import("./email").Mailer;
  // Keeps work such as sending mail running after the answer has gone out.
  defer(task: Promise<unknown>): void;
  // Rounds of password hashing, for work that has to take as long as the real thing.
  stages?(): number;
  // Takes one of today's mails from the budget of the whole service; false once it is spent.
  mailBudget?(): Promise<boolean>;
  // Whether an account still stands, for an address that names it.
  accountExists?(id: string): Promise<boolean>;
  now?(): number;
}
