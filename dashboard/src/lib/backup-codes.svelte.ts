// Freshly issued 2FA backup codes, kept in memory until the user confirms
// they saved them. They survive tab switches and in-app navigation (never
// written to storage); leaving the page asks for confirmation while they're
// unacknowledged.

let pending = $state<string[]>([]);

export function getPendingBackupCodes(): string[] {
	return pending;
}

export function setPendingBackupCodes(codes: string[]) {
	pending = [...codes];
}

export function acknowledgeBackupCodes() {
	pending = [];
}
