Feature: Reconnect sync

  @e2e
  Scenario: A peer that drops and reconnects catches up
    Given three peers editing through the chaos proxy, which drops, delays and duplicates frames
    When one peer's connection drops and it keeps editing offline, then reconnects
    Then all peers end with identical hashes
