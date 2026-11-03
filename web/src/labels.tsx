import type { ProgressPhase, TaskState } from "./lib/ipc/types";

/**
 * 任务状态与阶段的用户可见文案。
 * 抽到公共模块的原因：C09 工作台与「任务与撤销」视图都要显示同一套措辞，
 * 两处各写一份必然会漂移（同一个 state 在两个页面叫不同名字）。
 */
export function phaseLabel(phase: ProgressPhase): string {
  switch (phase) {
    case "queued":
      return "排队中";
    case "scanning":
      return "扫描中";
    case "planning":
      return "生成计划";
    case "executing":
      return "执行中";
    case "journaling":
      return "回滚 / 写 Journal";
    case "finalizing":
      return "收尾";
  }
}

export function stateBadge(state: TaskState) {
  if (state === "completed") return <span className="badge badge-safe">已完成</span>;
  if (state === "partial-failed") return <span className="badge badge-warn">部分失败</span>;
  if (state === "cancelled") return <span className="badge badge-warn">已取消</span>;
  if (state === "failed") return <span className="badge badge-danger">失败</span>;
  if (state === "paused") return <span className="badge badge-warn">已暂停</span>;
  if (state === "cancelling") return <span className="badge badge-warn">取消中</span>;
  if (state === "interrupted") return <span className="badge badge-warn">已中断</span>;
  if (state === "running") return <span className="badge">执行中</span>;
  return <span className="badge">排队中</span>;
}

/** 任务是否仍在推进（非终态）。 */
export const ACTIVE_STATES: TaskState[] = ["queued", "running", "paused", "cancelling"];

export function isActiveState(state: TaskState): boolean {
  return ACTIVE_STATES.includes(state);
}
