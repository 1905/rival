package review

// ReviewerOutput is the structured JSON every reviewer must emit.
type ReviewerOutput struct {
	Summary  string            `json:"summary"`
	Findings []ReviewerFinding `json:"findings"`
}

// ReviewerFinding is a single finding from one reviewer.
type ReviewerFinding struct {
	File     string `json:"file"`
	Line     int    `json:"line"`
	Severity string `json:"severity"`
	Category string `json:"category"`
	Title    string `json:"title"`
	Body     string `json:"body"`
	// FailureScenario is the input or state that triggers the defect and the
	// wrong result. Older payloads and the plan schema do not carry it.
	FailureScenario string `json:"failure_scenario,omitempty"`
	Suggestion      string `json:"suggestion,omitempty"`
	Confidence      int    `json:"confidence"`
}
