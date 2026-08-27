import type { ProviderView } from "./api";

export const ENTERPRISE_PROVIDER_ID = "tokenstation";
export const ENTERPRISE_PROVIDER_NAME = "Token-station";

export function providerDisplayName(
  provider: Pick<ProviderView, "name" | "managed_route">,
): string {
  return provider.managed_route && provider.name === ENTERPRISE_PROVIDER_ID
    ? ENTERPRISE_PROVIDER_NAME
    : provider.name;
}
