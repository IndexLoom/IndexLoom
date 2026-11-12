import type { FlareSolverrUpdate, IndexerGroupUpdate, OutboundProxyUpdate } from "../../lib/api";
export const EMPTY_CONNECTOR: ConnectorDraft = {
  id: "",
  name: "",
  endpoint: "http://flaresolverr:8191/v1",
  enabled: true,
  timeoutSeconds: 60,
};

export type ConnectorDraft = FlareSolverrUpdate & { id: string };
export type ProxyDraft = OutboundProxyUpdate & { id: string };
export type GroupDraft = IndexerGroupUpdate & { id: string };

export const EMPTY_PROXY: ProxyDraft = {
  id: "",
  name: "",
  kind: "http",
  host: "",
  port: 8080,
  enabled: true,
  username: null,
  clearPassword: false,
};

export const EMPTY_GROUP: GroupDraft = {
  id: "",
  name: "",
  description: "",
  enabled: true,
  priority: 0,
  proxyId: null,
};

