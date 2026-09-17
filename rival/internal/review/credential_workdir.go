package review

import "context"

type credentialWorkdirKey struct{}

// WithCredentialWorkdir keeps provider credential lookup in the caller's
// project when the reviewed files live in a temporary MR checkout.
func WithCredentialWorkdir(ctx context.Context, workdir string) context.Context {
	return context.WithValue(ctx, credentialWorkdirKey{}, workdir)
}

func credentialWorkdir(ctx context.Context, fallback string) string {
	if dir, ok := ctx.Value(credentialWorkdirKey{}).(string); ok && dir != "" {
		return dir
	}
	return fallback
}
