// Contents of the downloadable 2FA backup-code file.

/** Plain-text file contents for "Download". */
export function backupCodesFile(codes: string[], account?: string, now = new Date()): string {
	return [
		`Tond two-factor backup codes${account ? ` for ${account}` : ''}`,
		`Generated ${now.toISOString()}`,
		'Each code works once. Keep this file somewhere safe.',
		'',
		...codes
	].join('\n');
}
