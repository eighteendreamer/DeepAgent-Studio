declare module "zmodem.js" {
  export interface Offer {
    get_details(): { name: string; size: number };
    accept(options: { on_input: (bytes: number[]) => void }): Promise<unknown>;
    skip(): void;
  }
  export interface Transfer { send(bytes: Uint8Array): void; end(bytes: number[]): Promise<void> }
  export interface Session {
    type: "receive" | "send";
    on(event: "offer", handler: (offer: Offer) => void): void;
    on(event: "session_end", handler: () => void): void;
    start(): void;
    abort(): void;
    send_offer(details: { name: string; size: number }): Promise<Transfer | undefined>;
    close(): Promise<void>;
  }
  export interface Detection { get_session_role(): "receive" | "send"; confirm(): Session; deny(): void }
  class Sentry {
    constructor(options: { to_terminal(bytes: number[]): void; sender(bytes: number[]): void; on_detect(detection: Detection): void; on_retract(): void });
    consume(bytes: Uint8Array): void;
  }
  const Zmodem: { Sentry: typeof Sentry };
  export default Zmodem;
}
