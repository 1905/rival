package session

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"sort"
	"strings"
	"testing"
	"time"
)

// The session record contract shared with the Rust port
// (crates/rival-core/tests/contract.rs). testdata/expected.json at the repo
// root holds the typed values; testdata/written/ holds the Rust writer's
// golden bytes, which Go's json.MarshalIndent must reproduce exactly. Run with
// the release toolchain: GOTOOLCHAIN=go1.25.14.

// contractTime is [year, month, day, hour, minute, second, nanosecond,
// utc_offset_seconds].
type contractTime [8]int64

type contractFields struct {
	ID            string        `json:"id"`
	GroupID       string        `json:"group_id"`
	CLI           string        `json:"cli"`
	Mode          string        `json:"mode"`
	Model         string        `json:"model"`
	Effort        string        `json:"effort"`
	ReviewScope   string        `json:"review_scope"`
	Prompt        string        `json:"prompt"`
	PromptPreview string        `json:"prompt_preview"`
	PromptHash    string        `json:"prompt_hash"`
	Status        string        `json:"status"`
	StartTime     contractTime  `json:"start_time"`
	QueuedAt      *contractTime `json:"queued_at"`
	QueuePosition int64         `json:"queue_position"`
	EndTime       *contractTime `json:"end_time"`
	ExitCode      *int64        `json:"exit_code"`
	Duration      string        `json:"duration"`
	WorkDir       string        `json:"work_dir"`
	LogFile       string        `json:"log_file"`
	OutputBytes   int64         `json:"output_bytes"`
	OutputLines   int64         `json:"output_lines"`
	ErrorMsg      string        `json:"error"`
	Account       string        `json:"account"`
	PID           int64         `json:"pid"`
	PIDStart      int64         `json:"pid_start"`
	OwnerPID      int64         `json:"owner_pid"`
	OwnerPIDStart int64         `json:"owner_pid_start"`
}

type contractCase struct {
	ViaNewQueued bool           `json:"via_new_queued"`
	Fields       contractFields `json:"fields"`
}

type contractExpected struct {
	Doc      []string                  `json:"_doc"`
	Sessions map[string]contractFields `json:"sessions"`
	Written  map[string]contractCase   `json:"written"`
}

func contractTestdata(t *testing.T) string {
	t.Helper()
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller failed")
	}
	return filepath.Join(filepath.Dir(file), "..", "..", "..", "testdata")
}

func readContractExpected(t *testing.T) (contractExpected, []byte) {
	t.Helper()
	data, err := os.ReadFile(filepath.Join(contractTestdata(t), "expected.json"))
	if err != nil {
		t.Fatal(err)
	}
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.DisallowUnknownFields()
	var exp contractExpected
	if err := dec.Decode(&exp); err != nil {
		t.Fatalf("expected.json: %v", err)
	}
	return exp, data
}

func contractTimeOf(c contractTime) time.Time {
	loc := time.UTC
	if c[7] != 0 {
		loc = time.FixedZone("", int(c[7]))
	}
	return time.Date(int(c[0]), time.Month(c[1]), int(c[2]), int(c[3]), int(c[4]), int(c[5]), int(c[6]), loc)
}

func contractPartsOf(t time.Time) contractTime {
	y, mo, d := t.Date()
	h, mi, s := t.Clock()
	_, off := t.Zone()
	return contractTime{int64(y), int64(mo), int64(d), int64(h), int64(mi), int64(s), int64(t.Nanosecond()), int64(off)}
}

func contractSession(f contractFields) *Session {
	s := &Session{
		ID:            f.ID,
		GroupID:       f.GroupID,
		CLI:           f.CLI,
		Mode:          f.Mode,
		Model:         f.Model,
		Effort:        f.Effort,
		ReviewScope:   f.ReviewScope,
		Prompt:        f.Prompt,
		PromptPreview: f.PromptPreview,
		PromptHash:    f.PromptHash,
		Status:        f.Status,
		StartTime:     contractTimeOf(f.StartTime),
		QueuePosition: int(f.QueuePosition),
		Duration:      f.Duration,
		WorkDir:       f.WorkDir,
		LogFile:       f.LogFile,
		OutputBytes:   f.OutputBytes,
		OutputLines:   int(f.OutputLines),
		ErrorMsg:      f.ErrorMsg,
		Account:       f.Account,
		PID:           int(f.PID),
		PIDStart:      f.PIDStart,
		OwnerPID:      int(f.OwnerPID),
		OwnerPIDStart: f.OwnerPIDStart,
	}
	if f.QueuedAt != nil {
		q := contractTimeOf(*f.QueuedAt)
		s.QueuedAt = &q
	}
	if f.EndTime != nil {
		e := contractTimeOf(*f.EndTime)
		s.EndTime = &e
	}
	if f.ExitCode != nil {
		c := int(*f.ExitCode)
		s.ExitCode = &c
	}
	return s
}

func contractFieldsOf(s *Session) contractFields {
	f := contractFields{
		ID:            s.ID,
		GroupID:       s.GroupID,
		CLI:           s.CLI,
		Mode:          s.Mode,
		Model:         s.Model,
		Effort:        s.Effort,
		ReviewScope:   s.ReviewScope,
		Prompt:        s.Prompt,
		PromptPreview: s.PromptPreview,
		PromptHash:    s.PromptHash,
		Status:        s.Status,
		StartTime:     contractPartsOf(s.StartTime),
		QueuePosition: int64(s.QueuePosition),
		Duration:      s.Duration,
		WorkDir:       s.WorkDir,
		LogFile:       s.LogFile,
		OutputBytes:   s.OutputBytes,
		OutputLines:   int64(s.OutputLines),
		ErrorMsg:      s.ErrorMsg,
		Account:       s.Account,
		PID:           int64(s.PID),
		PIDStart:      s.PIDStart,
		OwnerPID:      int64(s.OwnerPID),
		OwnerPIDStart: s.OwnerPIDStart,
	}
	if s.QueuedAt != nil {
		q := contractPartsOf(*s.QueuedAt)
		f.QueuedAt = &q
	}
	if s.EndTime != nil {
		e := contractPartsOf(*s.EndTime)
		f.EndTime = &e
	}
	if s.ExitCode != nil {
		c := int64(*s.ExitCode)
		f.ExitCode = &c
	}
	return f
}

func contractWrite(t *testing.T, s *Session) []byte {
	t.Helper()
	data, err := json.MarshalIndent(s, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	return data
}

// contractWriteViaNewQueued writes the record NewQueued builds for these
// inputs, with every field except the generated PromptPreview and PromptHash
// taken from f. The caller has pointed HOME at a temp dir.
func contractWriteViaNewQueued(t *testing.T, f contractFields) []byte {
	t.Helper()
	q, err := NewQueued(f.CLI, f.Mode, f.Model, f.Effort, f.WorkDir, f.Prompt, f.ReviewScope, f.GroupID)
	if err != nil {
		t.Fatal(err)
	}
	if q.PromptHash != f.PromptHash {
		t.Errorf("%s: prompt_hash %q, want %q", f.ID, q.PromptHash, f.PromptHash)
	}
	s := contractSession(f)
	s.PromptPreview = q.PromptPreview
	s.PromptHash = q.PromptHash
	return contractWrite(t, s)
}

func contractResavedName(name string) string {
	return strings.TrimSuffix(name, ".json") + ".resaved.json"
}

// contractGolden returns every file under testdata/written/ as Go writes it.
func contractGolden(t *testing.T, exp contractExpected) map[string][]byte {
	t.Helper()
	t.Setenv("HOME", t.TempDir())
	out := map[string][]byte{}
	for name, f := range exp.Sessions {
		out[name] = contractWrite(t, contractSession(f))
	}
	for name, c := range exp.Written {
		if c.ViaNewQueued {
			out[name] = contractWriteViaNewQueued(t, c.Fields)
			out[contractResavedName(name)] = contractWrite(t, contractSession(c.Fields))
		} else {
			out[name] = contractWrite(t, contractSession(c.Fields))
		}
	}
	return out
}

func readContractGolden(t *testing.T, name string) []byte {
	t.Helper()
	data, err := os.ReadFile(filepath.Join(contractTestdata(t), "written", name))
	if err != nil {
		t.Fatalf("%v; the Rust contract test regenerates testdata/written", err)
	}
	return data
}

func assertContractBytes(t *testing.T, what string, got, want []byte) {
	t.Helper()
	if !bytes.Equal(got, want) {
		t.Errorf("%s: bytes differ (%s; the contract targets the release toolchain go1.25)\n--- got\n%q\n--- want\n%q",
			what, runtime.Version(), got, want)
	}
}

func contractDecode(t *testing.T, what string, data []byte) *Session {
	t.Helper()
	var s Session
	if err := json.Unmarshal(data, &s); err != nil {
		t.Fatalf("%s: decode: %v", what, err)
	}
	return &s
}

func contractDirNames(t *testing.T, dir string) []string {
	t.Helper()
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	var names []string
	for _, e := range entries {
		names = append(names, e.Name())
	}
	return names
}

func sortedKeys[V any](m map[string]V) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

// Every fields object names every Session JSON field, so a new field cannot
// slip past the contract.
func TestContractExpectedListsEverySessionField(t *testing.T) {
	_, data := readContractExpected(t)
	var raw struct {
		Sessions map[string]map[string]json.RawMessage `json:"sessions"`
		Written  map[string]struct {
			Fields map[string]json.RawMessage `json:"fields"`
		} `json:"written"`
	}
	if err := json.Unmarshal(data, &raw); err != nil {
		t.Fatal(err)
	}
	var want []string
	st := reflect.TypeOf(Session{})
	for i := 0; i < st.NumField(); i++ {
		want = append(want, strings.Split(st.Field(i).Tag.Get("json"), ",")[0])
	}
	sort.Strings(want)
	objects := map[string]map[string]json.RawMessage{}
	for name, f := range raw.Sessions {
		objects["sessions/"+name] = f
	}
	for name, c := range raw.Written {
		objects["written/"+name] = c.Fields
	}
	for name, f := range objects {
		if got := sortedKeys(f); !reflect.DeepEqual(got, want) {
			t.Errorf("%s keys = %v, want %v", name, got, want)
		}
	}
}

func TestContractCorpusFilesMatchExpected(t *testing.T) {
	exp, _ := readContractExpected(t)
	if got, want := contractDirNames(t, filepath.Join(contractTestdata(t), "sessions")), sortedKeys(exp.Sessions); !reflect.DeepEqual(got, want) {
		t.Errorf("testdata/sessions = %v, want %v", got, want)
	}
	if got, want := contractDirNames(t, filepath.Join(contractTestdata(t), "written")), sortedKeys(contractGolden(t, exp)); !reflect.DeepEqual(got, want) {
		t.Errorf("testdata/written = %v, want %v", got, want)
	}
}

func TestContractFixturesDecodeToExpected(t *testing.T) {
	exp, _ := readContractExpected(t)
	for _, name := range sortedKeys(exp.Sessions) {
		data, err := os.ReadFile(filepath.Join(contractTestdata(t), "sessions", name))
		if err != nil {
			t.Fatal(err)
		}
		if got, want := contractFieldsOf(contractDecode(t, name, data)), exp.Sessions[name]; !reflect.DeepEqual(got, want) {
			t.Errorf("%s:\n got %+v\nwant %+v", name, got, want)
		}
	}
}

// Go writes a session from the expected values; the bytes equal the Rust
// writer's golden output for the same input.
func TestContractWriterMatchesRustGolden(t *testing.T) {
	exp, _ := readContractExpected(t)
	golden := contractGolden(t, exp)
	for _, name := range sortedKeys(golden) {
		assertContractBytes(t, name, golden[name], readContractGolden(t, name))
	}
}

func TestContractFixtureLoadSaveMatchesGolden(t *testing.T) {
	exp, _ := readContractExpected(t)
	for _, name := range sortedKeys(exp.Sessions) {
		data, err := os.ReadFile(filepath.Join(contractTestdata(t), "sessions", name))
		if err != nil {
			t.Fatal(err)
		}
		assertContractBytes(t, name, contractWrite(t, contractDecode(t, name, data)), readContractGolden(t, name))
	}
}

// Load a golden record, save it, load that again. A split rune's bytes were
// written as \ufffd escapes; they load as a valid U+FFFD, which the next save
// writes literally.
func TestContractGoldenLoadSaveLoad(t *testing.T) {
	exp, _ := readContractExpected(t)
	type item struct {
		fields contractFields
		via    bool
	}
	items := map[string]item{}
	for name, f := range exp.Sessions {
		items[name] = item{f, false}
	}
	for name, c := range exp.Written {
		items[name] = item{c.Fields, c.ViaNewQueued}
	}
	for _, name := range sortedKeys(items) {
		it := items[name]
		loaded := contractDecode(t, name, readContractGolden(t, name))
		if got := contractFieldsOf(loaded); !reflect.DeepEqual(got, it.fields) {
			t.Errorf("%s decoded:\n got %+v\nwant %+v", name, got, it.fields)
		}
		resavedFile := name
		if it.via {
			resavedFile = contractResavedName(name)
		}
		resaved := contractWrite(t, loaded)
		assertContractBytes(t, name+" resaved", resaved, readContractGolden(t, resavedFile))
		again := contractDecode(t, resavedFile, resaved)
		if got := contractFieldsOf(again); !reflect.DeepEqual(got, it.fields) {
			t.Errorf("%s decoded:\n got %+v\nwant %+v", resavedFile, got, it.fields)
		}
		assertContractBytes(t, resavedFile+" resaved", contractWrite(t, again), resaved)
	}
}

func TestContractSplitPreviewBytesAreEscapesThenLiteral(t *testing.T) {
	line := func(data []byte) string {
		for _, l := range strings.Split(string(data), "\n") {
			if strings.HasPrefix(l, `  "prompt_preview"`) {
				return l
			}
		}
		t.Fatal("no prompt_preview line")
		return ""
	}
	first := line(readContractGolden(t, "preview-split-4byte.json"))
	if !strings.HasSuffix(first, `x\ufffd\ufffd",`) {
		t.Errorf("first save must escape each stray byte: %q", first)
	}
	if !strings.HasPrefix(first, "  \"prompt_preview\": \"Literal \uFFFD kept.") {
		t.Errorf("a literal U+FFFD in the prompt must stay literal: %q", first)
	}
	if resaved := line(readContractGolden(t, "preview-split-4byte.resaved.json")); !strings.HasSuffix(resaved, "x\uFFFD\uFFFD\",") {
		t.Errorf("a loaded U+FFFD must be written literally: %q", resaved)
	}
}
