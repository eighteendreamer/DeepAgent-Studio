process.stdout.write(
  `${JSON.stringify({
    type: "item.updated",
    threadId: "thread-cli",
    turnId: "turn-cli",
    itemId: null,
    item: { kind: "content_delta", text: "cli output" }
  })}\n`
);
process.stdout.write(
  `${JSON.stringify({
    type: "turn.completed",
    threadId: "thread-cli",
    turnId: "turn-cli",
    message: "done"
  })}\n`
);
