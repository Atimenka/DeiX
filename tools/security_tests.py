#!/usr/bin/env python3
"""
DeiX Security Audit Test Script
Tests all confirmed vulnerabilities are fixed.

Usage: python3 security_tests.py
Requires: requests library (pip install requests)
"""

import json
import sys
import urllib.request
import urllib.error

SUPABASE_URL = "https://xtsrvmwfnsxunvwuaplz.supabase.co"
ANON_KEY = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6Inh0c3J2bXdmbnN4dW52d3VhcGx6Iiwicm9sZSI6ImFub24iLCJpYXQiOjE3OTIyMjI1NzQsImV4cCI6MjEwNzc5ODU3NH0.J3AAU7crZ3NZO9MjpN-ie56N76amsi_VNala1ZlKZAI"
SERVICE_KEY = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6Inh0c3J2bXdmbnN4dW52d3VhcGx6Iiwicm9sZSI6InNlcnZpY2Vfcm9sZSIsImlhdCI6MTc5MjIyMjU3NCwiZXhwIjoyMTA3Nzk4NTc0fQ.MRseJco6RUhTW8-s8cLZJOX9fRUiaa3lG9ouDhtqpL8"

TESTS_PASSED = 0
TESTS_FAILED = 0


def http_request(method, path, key=ANON_KEY, body=None, headers=None):
    """Make HTTP request to Supabase REST API."""
    url = f"{SUPABASE_URL}/rest/v1{path}"
    all_headers = {
        "apikey": key,
        "Authorization": f"Bearer {key}",
        "Content-Type": "application/json",
    }
    if headers:
        all_headers.update(headers)
    data = json.dumps(body).encode("utf-8") if body is not None else None
    req = urllib.request.Request(url, data=data, method=method, headers=all_headers)
    try:
        with urllib.request.urlopen(req) as resp:
            return resp.status, json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read().decode("utf-8")) if e.read() else {}
    except Exception as e:
        return 0, {"error": str(e)}


def test(name, condition, detail=""):
    """Run a test and report result."""
    global TESTS_PASSED, TESTS_FAILED
    if condition:
        TESTS_PASSED += 1
        print(f"  [PASS] {name}")
    else:
        TESTS_FAILED += 1
        print(f"  [FAIL] {name} — {detail}")


def test_anon_roles_insert():
    """Test: anon cannot INSERT into roles table."""
    print("\n=== Test: anon INSERT on roles ===")
    status, _ = http_request("POST", "/roles", body={
        "name": "hacked_role",
        "description": "Should be rejected",
        "rank": 9999,
    })
    test("anon INSERT on roles blocked", status in (401, 403), f"got HTTP {status}")


def test_anon_roles_update():
    """Test: anon cannot UPDATE roles table."""
    print("\n=== Test: anon UPDATE on roles ===")
    status, _ = http_request("PATCH", "/roles?name=eq.owner", body={"rank": 999})
    test("anon UPDATE on roles blocked", status in (401, 403), f"got HTTP {status}")


def test_anon_roles_delete():
    """Test: anon cannot DELETE from roles table."""
    print("\n=== Test: anon DELETE on roles ===")
    status, _ = http_request("DELETE", "/roles?name=eq.tester")
    test("anon DELETE on roles blocked", status in (401, 403), f"got HTTP {status}")


def test_anon_roles_rank_change():
    """Test: anon cannot change rank of existing roles."""
    print("\n=== Test: anon rank change on roles ===")
    status, _ = http_request("PATCH", "/roles?name=eq.tester", body={"rank": 9999})
    test("anon rank change blocked", status in (401, 403), f"got HTTP {status}")


def test_anon_roles_select():
    """Test: anon cannot SELECT from roles table (no policy)."""
    print("\n=== Test: anon SELECT on roles ===")
    status, data = http_request("GET", "/roles")
    # After fix: anon should get 401 or 403 (no RLS policy)
    # Before fix: anon could see all roles
    test("anon SELECT on roles blocked", status in (401, 403), f"got HTTP {status}")


def test_anon_claim_dev_unlock():
    """Test: anon cannot call claim_dev_unlock RPC."""
    print("\n=== Test: anon claim_dev_unlock RPC ===")
    status, _ = http_request("POST", "/rpc/claim_dev_unlock", body={
        "p_device_id": "0123456789abcdef0123456789abcdef",
    })
    test("anon claim_dev_unlock blocked", status in (401, 403), f"got HTTP {status}")


def test_anon_request_dev_unlock():
    """Test: anon cannot call request_dev_unlock RPC."""
    print("\n=== Test: anon request_dev_unlock RPC ===")
    status, _ = http_request("POST", "/rpc/request_dev_unlock", body={
        "p_device_id": "0123456789abcdef0123456789abcdef",
        "p_source": "os",
    })
    test("anon request_dev_unlock blocked", status in (401, 403), f"got HTTP {status}")


def test_anon_begin_device_link():
    """Test: anon CAN call begin_device_link (OS needs it)."""
    print("\n=== Test: anon begin_device_link (should work) ===")
    status, data = http_request("POST", "/rpc/begin_device_link", body={
        "p_device_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    })
    test("anon begin_device_link works", status == 200, f"got HTTP {status}")


def test_anon_feature_flags_read():
    """Test: anon can read feature_flags (OS needs it)."""
    print("\n=== Test: anon feature_flags SELECT (should work) ===")
    status, data = http_request("GET", "/feature_flags")
    test("anon feature_flags read works", status == 200 and isinstance(data, list), f"got HTTP {status}")


def test_anon_profiles_read():
    """Test: anon cannot read profiles table."""
    print("\n=== Test: anon profiles SELECT ===")
    status, _ = http_request("GET", "/profiles")
    test("anon profiles SELECT blocked", status in (401, 403), f"got HTTP {status}")


def test_anon_user_roles_read():
    """Test: anon cannot read user_roles table (no ACL grant)."""
    print("\n=== Test: anon user_roles SELECT ===")
    status, _ = http_request("GET", "/user_roles")
    test("anon user_roles SELECT blocked", status in (401, 403), f"got HTTP {status}")


def test_anon_dev_keys_read():
    """Test: anon cannot read dev_keys table."""
    print("\n=== Test: anon dev_keys SELECT ===")
    status, _ = http_request("GET", "/dev_keys")
    test("anon dev_keys SELECT blocked", status in (401, 403), f"got HTTP {status}")


def test_anon_mfa_backup_codes_read():
    """Test: anon cannot read mfa_backup_codes table."""
    print("\n=== Test: anon mfa_backup_codes SELECT ===")
    status, _ = http_request("GET", "/mfa_backup_codes")
    test("anon mfa_backup_codes SELECT blocked", status in (401, 403), f"got HTTP {status}")


def test_service_role_roles_manage():
    """Test: service_role can still manage roles (backend functionality)."""
    print("\n=== Test: service_role roles management ===")
    # Read roles
    status, data = http_request("GET", "/roles", key=SERVICE_KEY)
    test("service_role can SELECT roles", status == 200 and isinstance(data, list), f"got HTTP {status}")

    # Insert a test role
    status, data = http_request("POST", "/roles", key=SERVICE_KEY, body={
        "name": "test_role_security",
        "description": "Test role for security audit",
        "rank": 5,
    })
    test("service_role can INSERT role", status in (200, 201), f"got HTTP {status}")

    # Clean up
    if status in (200, 201) and isinstance(data, list) and data:
        status2, _ = http_request("DELETE", "/roles?name=eq.test_role_security", key=SERVICE_KEY)
        test("service_role can DELETE test role", status2 in (200, 204), f"got HTTP {status2}")


def test_service_role_claim_dev_unlock():
    """Test: service_role can call claim_dev_unlock with x-device-id header."""
    print("\n=== Test: service_role claim_dev_unlock ===")
    device_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    # First create a request via service_role
    status, _ = http_request("POST", "/rpc/request_dev_unlock", key=SERVICE_KEY, body={
        "p_device_id": device_id,
        "p_source": "os",
    }, headers={"x-device-id": device_id})
    # The device is not linked, so this should fail
    test("service_role request_dev_unlock without linked device fails", status >= 400, f"got HTTP {status}")

    # Test claim_dev_unlock without x-device-id header
    status, data = http_request("POST", "/rpc/claim_dev_unlock", key=SERVICE_KEY, body={
        "p_device_id": device_id,
    })
    test("service_role claim_dev_unlock without header fails", status >= 400, f"got HTTP {status}")

    # Test claim_dev_unlock with matching x-device-id header
    status, data = http_request("POST", "/rpc/claim_dev_unlock", key=SERVICE_KEY, body={
        "p_device_id": device_id,
    }, headers={"x-device-id": device_id})
    # Should return 'none' status (device not found) not 'device identity mismatch'
    test("service_role claim_dev_unlock with header returns valid response", status == 200, f"got HTTP {status}")


def test_begin_device_link_rate_limit():
    """Test: begin_device_link rate limiting (3 codes per device per hour)."""
    print("\n=== Test: begin_device_link rate limit ===")
    device_id = "cccccccccccccccccccccccccccccccc"
    results = []
    for i in range(4):
        status, data = http_request("POST", "/rpc/begin_device_link", body={
            "p_device_id": device_id,
        })
        results.append((status, data))

    # First 3 should succeed, 4th should fail
    success_count = sum(1 for s, _ in results if s == 200)
    test("begin_device_link rate limit (max 3 per hour)", success_count <= 3, f"got {success_count} successes out of 4")


def main():
    print("=" * 60)
    print("DeiX Security Audit Test Script")
    print("=" * 60)
    print(f"Supabase URL: {SUPABASE_URL}")
    print()

    # Run all tests
    test_anon_roles_insert()
    test_anon_roles_update()
    test_anon_roles_delete()
    test_anon_roles_rank_change()
    test_anon_roles_select()
    test_anon_claim_dev_unlock()
    test_anon_request_dev_unlock()
    test_anon_begin_device_link()
    test_anon_feature_flags_read()
    test_anon_profiles_read()
    test_anon_user_roles_read()
    test_anon_dev_keys_read()
    test_anon_mfa_backup_codes_read()
    test_service_role_roles_manage()
    test_service_role_claim_dev_unlock()
    test_begin_device_link_rate_limit()

    print()
    print("=" * 60)
    print(f"Results: {TESTS_PASSED} passed, {TESTS_FAILED} failed")
    print("=" * 60)

    if TESTS_FAILED > 0:
        sys.exit(1)
    else:
        print("All security tests passed!")
        sys.exit(0)


if __name__ == "__main__":
    main()
