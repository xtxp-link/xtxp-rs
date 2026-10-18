Feature: Late joiner

  @e2e
  Scenario: A late joiner loads the full state in one update
    Given three peers have edited a drawing through the host
    When a fourth peer connects
    Then it receives the whole document in one update and its hash matches theirs
