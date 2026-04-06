import { describe, test, expect } from "bun:test";

describe("send lock ordering", () => {
  test("concurrent calls are serialized per chat", async () => {
    const order: number[] = [];
    const lock = new Map<number, Promise<void>>();

    async function sendWithLock(chatId: number, id: number, delayMs: number) {
      const prev = lock.get(chatId) ?? Promise.resolve();
      const current = prev.then(async () => {
        await new Promise((r) => setTimeout(r, delayMs));
        order.push(id);
      });
      lock.set(chatId, current.catch(() => {}));
      await current;
    }

    const p1 = sendWithLock(123, 1, 30);
    const p2 = sendWithLock(123, 2, 10);
    await Promise.all([p1, p2]);

    expect(order).toEqual([1, 2]);
  });

  test("different chats are independent", async () => {
    const order: string[] = [];
    const lock = new Map<number, Promise<void>>();

    async function sendWithLock(chatId: number, id: string, delayMs: number) {
      const prev = lock.get(chatId) ?? Promise.resolve();
      const current = prev.then(async () => {
        await new Promise((r) => setTimeout(r, delayMs));
        order.push(id);
      });
      lock.set(chatId, current.catch(() => {}));
      await current;
    }

    const pA = sendWithLock(111, "A", 40);
    const pB = sendWithLock(222, "B", 10);
    await Promise.all([pA, pB]);

    expect(order).toEqual(["B", "A"]);
  });
});
