package parser

import (
	"fmt"
	"strings"

	"github.com/1905/rival/internal/config"
)

// ParseReviewArgs parses raw review arguments (used by antislop).
// Grammar: [options] [scope] — always a review, no "review" keyword needed.
// Options may appear before or after scope tokens:
//
//	-re, --effort <level>
//	-m, --model <selector[,selector...]>
//
// Model options may be repeated. "--" ends option parsing so a scope beginning
// with a dash can still be reviewed.
func ParseReviewArgs(raw string) (*ParseResult, error) {
	s := strings.TrimSpace(raw)
	result := &ParseResult{IsReview: true}
	var scopeParts []string

	for s != "" {
		token, rest := popReviewToken(s)
		if token == "--" {
			result.Escaped = true
			if rest = strings.TrimSpace(rest); rest != "" {
				scopeParts = append(scopeParts, rest)
			}
			break
		}

		name, inlineValue, hasInlineValue := splitReviewOption(token)
		switch name {
		case "-h", "--help":
			result.IsEmpty = true
			return result, nil
		case "-re", "--effort":
			value, remaining, err := reviewOptionValue(name, inlineValue, hasInlineValue, rest)
			if err != nil {
				return nil, err
			}
			if !config.IsValidEffort(value) {
				return nil, fmt.Errorf("invalid effort level %q, must be one of: %s", value, strings.Join(config.ValidEfforts, ", "))
			}
			result.Effort = value
			s = remaining
		case "-m", "--model":
			value, remaining, err := reviewOptionValue(name, inlineValue, hasInlineValue, rest)
			if err != nil {
				return nil, err
			}
			models, err := splitModelValues(value)
			if err != nil {
				return nil, err
			}
			result.Models = append(result.Models, models...)
			s = remaining
		default:
			if strings.HasPrefix(token, "-") {
				return nil, fmt.Errorf("unknown review option %q; use -m/--model, -re/--effort, or -- before a scope beginning with '-'", token)
			}
			scopeParts = append(scopeParts, token)
			s = strings.TrimSpace(rest)
		}
	}

	scope := strings.Join(scopeParts, " ")
	if scope == "" {
		result.AutoScope = true
		scope = config.WholeProject
	}
	result.ReviewScope = scope
	return result, nil
}

func popReviewToken(s string) (token, rest string) {
	s = strings.TrimLeft(s, " \t\r\n")
	if i := strings.IndexAny(s, " \t\r\n"); i >= 0 {
		return s[:i], strings.TrimLeft(s[i:], " \t\r\n")
	}
	return s, ""
}

func splitReviewOption(token string) (name, value string, hasValue bool) {
	if i := strings.Index(token, "="); i >= 0 {
		return token[:i], token[i+1:], true
	}
	return token, "", false
}

func reviewOptionValue(name, inlineValue string, hasInlineValue bool, rest string) (value, remaining string, err error) {
	if hasInlineValue {
		if strings.TrimSpace(inlineValue) == "" {
			return "", "", fmt.Errorf("option %s requires a value", name)
		}
		return strings.TrimSpace(inlineValue), strings.TrimSpace(rest), nil
	}
	if strings.TrimSpace(rest) == "" {
		return "", "", fmt.Errorf("option %s requires a value", name)
	}
	value, remaining = popReviewToken(rest)
	if strings.HasPrefix(value, "-") {
		return "", "", fmt.Errorf("option %s requires a value", name)
	}
	return value, strings.TrimSpace(remaining), nil
}

func splitModelValues(value string) ([]string, error) {
	parts := strings.Split(value, ",")
	models := make([]string, 0, len(parts))
	for _, part := range parts {
		part = strings.TrimSpace(part)
		if part == "" {
			return nil, fmt.Errorf("model selector cannot be empty")
		}
		models = append(models, part)
	}
	return models, nil
}
