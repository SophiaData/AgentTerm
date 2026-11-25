from __future__ import annotations

import sys
import unittest
from pathlib import Path
import importlib.util


def main() -> int:
    loader = unittest.defaultTestLoader
    root = Path(__file__).resolve().parents[1]
    test_file = root / "tests" / "test_template_contract.py"
    spec = importlib.util.spec_from_file_location("test_template_contract", test_file)
    if spec is None or spec.loader is None:
        print(f"Unable to load template contract: {test_file}", file=sys.stderr)
        return 1
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    suite = loader.loadTestsFromModule(module)
    count = suite.countTestCases()
    if count < 1:
        print("Template contract suite contains zero tests", file=sys.stderr)
        return 1
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    raise SystemExit(main())
