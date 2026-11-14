import { translate, formatDateTime, formatNumber } from "../../lib/i18n";
export const t = (message: string) => translate(message);
export const metric = (value: number | null) => value === null ? "—" : formatNumber(value);
export function timestamp(value: string) { return value && Number.isFinite(Date.parse(value)) ? formatDateTime(value) : "—"; }
export function Status({ value }: { value: string }) { return <span className="monitoring-status" data-status={value}>{t(value)}</span>; }
