import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Select, Option } from "../ui/Select";
import { configApi } from "../../api/client";
import { useToastStore } from "../../stores/toastStore";

export function StorageSection() {
  const queryClient = useQueryClient();
  const addToast = useToastStore((s) => s.addToast);

  const { data: retention } = useQuery({
    queryKey: ["retention"],
    queryFn: configApi.getRetention,
  });

  async function handleRetentionChange(days: number) {
    try {
      await configApi.setRetention({ group_message_max_age_days: days });
      await queryClient.invalidateQueries({ queryKey: ["retention"] });
    } catch (e) {
      addToast(String(e), "error");
    }
  }

  return (
    <div className="space-y-8">
      <div>
        <h1 className="text-lg font-semibold text-surface-900 dark:text-surface-50">Storage</h1>
        <p className="mt-1 text-sm text-surface-500">
          Configure how long messages are kept on this device.
        </p>
      </div>

      {/* Retention */}
      <section className="space-y-4">
        <h3 className="text-sm font-semibold text-surface-900 dark:text-surface-50">
          Message Retention
        </h3>

        <div className="max-w-xs">
          <Select
            label="Keep messages for"
            value={retention?.group_message_max_age_days ?? 30}
            onChange={(v) => void handleRetentionChange(Number(v))}
          >
            <Option value={0}>Keep forever</Option>
            <Option value={90}>90 days</Option>
            <Option value={30}>30 days (default)</Option>
            <Option value={14}>14 days</Option>
          </Select>
        </div>

        <p className="text-sm text-surface-400">
          Applies to both direct and group messages stored locally on this device.
        </p>
      </section>
    </div>
  );
}
