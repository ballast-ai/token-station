import type { ProviderView } from "./api";

export const ENTERPRISE_PROVIDER_ID = "tokenstation";
export const ENTERPRISE_PROVIDER_NAME = "Token-station";

export function providerDisplayName(
  provider: Pick<ProviderView, "name" | "managed_route">,
): string {
  if (!provider.managed_route) return provider.name;
  if (provider.name === ENTERPRISE_PROVIDER_ID) return ENTERPRISE_PROVIDER_NAME;
  const managedIndex = /^tokenstation_(\d+)$/.exec(provider.name)?.[1];
  return managedIndex ? `${ENTERPRISE_PROVIDER_NAME} ${managedIndex}` : provider.name;
}
