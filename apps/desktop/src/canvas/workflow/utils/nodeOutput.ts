/** 专业节点的执行结果兼容文本和 { success, outputs }，不回退到输入字段。 */
export function nodeResultText(result: unknown): string | undefined {
  if (result === undefined || result === null) return undefined;
  const output = typeof result === "object" && "outputs" in result ? result.outputs : result;
  if (output === undefined || output === null) return undefined;
  if (typeof output === "string") return output;
  if (typeof output === "object") {
    if ("text" in output && typeof output.text === "string") return output.text;
    if ("answer" in output && typeof output.answer === "string") return output.answer;
  }
  return JSON.stringify(output, null, 2);
}

export function nodeOutputMarkdown(label: string, output: string | undefined): string | null {
  return output ? `# ${label}\n\n${output}` : null;
}
