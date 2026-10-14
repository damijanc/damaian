// Package uploads is an accuracy fixture for the heuristic Go extractor. Read as text, never compiled.
package uploads

import (
	"fmt"
)

type Uploader struct {
	Path string
}

type Store interface {
	Put(key string)
}

type UploadID string

func UploadRetry(attempts int) bool {
	return attempts < 3
}

func (u *Uploader) Upload() error {
	return fmt.Errorf("not implemented")
}

/*
func blockCommented() {}
*/

const template = `
func insideRawString() {}
`
