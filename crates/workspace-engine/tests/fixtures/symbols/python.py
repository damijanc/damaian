"""Accuracy fixture for the heuristic Python extractor. Read as text, never executed.

def inside_module_docstring():
    pass
"""

import os
from .storage import save


class Uploader:
    """Uploads files.

    class InsideClassDocstring:
    """

    def upload(self, path):
        return save(path)


def upload_retry(attempts):
    def nested_backoff():
        return attempts * 2

    return nested_backoff()


async def fetch_remote():
    return os.getcwd()


# def commented_out():

TEMPLATE = '''
def inside_string():
'''
