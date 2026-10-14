<?php
// Accuracy fixture for the heuristic PHP extractor. Read as text, never executed.

class Uploader
{
    function upload($path)
    {
        return $path;
    }

    public function retry($attempts)
    {
        return $attempts < 3;
    }
}

function upload_retry($attempts)
{
    return $attempts < 3;
}

# function hash_commented() {}

/*
function block_commented() {}
*/

$template = '
function inside_string() {}
';
