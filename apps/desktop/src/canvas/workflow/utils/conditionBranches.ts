// v1 local projection of the same branch policy compiled into the Rust kernel.
// No condition evaluation or second execution path belongs in this module.
import * as policy from "../../../../../../crates/deepagent-runtime/src/workflow/condition-branches.v1.json";

export function conditionBranchContract(conditions: unknown) {
  if (policy.version !== 1 || !Array.isArray(conditions) || conditions.length === 0) {
    throw new Error("ConditionConfigInvalid: 至少需要一个条件分支");
  }
  const ids = new Set<string>();
  const cases = conditions.map((group: unknown) => {
    const id = group && typeof group === "object" ? (group as { id?: unknown }).id : undefined;
    // Same existing graph ID grammar; raw values never appear in diagnostics.
    if (typeof id !== "string" || !/^[A-Za-z0-9_-]{1,100}$/.test(id)) {
      throw new Error("ConditionConfigInvalid: 分支标识格式无效");
    }
    if (policy.reserved.includes(id) && !(conditions.length === 1 && id === policy.single_case.id)) {
      throw new Error("ConditionConfigInvalid: 分支标识与保留端口冲突");
    }
    if (ids.has(id)) throw new Error("ConditionConfigInvalid: 分支标识重复");
    ids.add(id);
    return conditions.length === 1 ? { ...policy.single_case } : { id, label: id.toUpperCase() };
  });
  return { version: policy.version, cases, fallback: { ...policy.fallback } };
}

/** Incomplete editor configurations stay editable, with an explicit error and no misleading ports. */
export function conditionBranchView(conditions: unknown) {
  try {
    const contract = conditionBranchContract(conditions);
    return { handles: [...contract.cases, contract.fallback], error: null };
  } catch (error) {
    return { handles: [], error: error instanceof Error ? error.message : "条件分支配置无效" };
  }
}
